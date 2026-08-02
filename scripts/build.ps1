<#
.SYNOPSIS
  Release build + packaging for Windows: a portable .exe and an .msi installer.

.DESCRIPTION
  The Unix side is scripts/build.sh; this is the half that needs MSVC and WiX.

  libmpv is the one thing that makes a Windows build of this repo different from
  `cargo build`. libmpv2-sys emits `cargo:rustc-link-lib=mpv`, so the linker
  wants an import library called mpv.lib, and the process wants libmpv-2.dll at
  run time. Neither ships with mpv's Windows *player* build — they come from the
  separate "mpv-dev" archive:

      https://sourceforge.net/projects/mpv-player-windows/files/libmpv/

  Unpack it somewhere and point this script at it with -MpvDev or $env:MPV_DEV_DIR.
  If it contains mpv.def but no mpv.lib, the import library is generated here
  with lib.exe.

  There is no static libmpv worth shipping, so "standalone .exe" means the exe
  plus libmpv-2.dll next to it — which is what the portable .zip contains.

.EXAMPLE
  scripts\build.ps1 -MpvDev C:\mpv-dev
  scripts\build.ps1 -Artifacts msi -SkipBuild
#>

[CmdletBinding()]
param(
  # Directory holding libmpv-2.dll plus mpv.lib or mpv.def.
  [string] $MpvDev = $env:MPV_DEV_DIR,

  # exe | zip | msi | all
  [ValidateSet('exe', 'zip', 'msi', 'all')]
  [string[]] $Artifacts = @('all'),

  # Reuse target\release from a previous run.
  [switch] $SkipBuild,

  # Rust target triple; the default is the host.
  [string] $Target = 'x86_64-pc-windows-msvc',

  # Authenticode signing, if you have a certificate. Both or neither.
  [string] $SignCertThumbprint,
  [string] $TimestampUrl = 'http://timestamp.digicert.com'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root    = Split-Path -Parent $PSScriptRoot
$Dist    = Join-Path $Root 'dist'
$OutDir  = Join-Path $Dist 'windows'
$Pkg     = Join-Path $Root 'packaging\windows'

$Version = (Select-String -Path (Join-Path $Root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' `
            | Select-Object -First 1).Matches[0].Groups[1].Value

function Log  ($m) { Write-Host "==> $m" -ForegroundColor Magenta }
function Warn ($m) { Write-Host "warning: $m" -ForegroundColor Yellow }
function Die  ($m) { Write-Host "error: $m" -ForegroundColor Red; exit 1 }

if ($Artifacts -contains 'all') { $Artifacts = @('exe', 'zip', 'msi') }

# ------------------------------------------------------------------ libmpv ---

function Resolve-MpvDev {
  if (-not $MpvDev) {
    Die @"
no libmpv development files. Download the mpv-dev archive from
  https://sourceforge.net/projects/mpv-player-windows/files/libmpv/
unpack it, and pass -MpvDev <dir> or set MPV_DEV_DIR.
"@
  }
  if (-not (Test-Path $MpvDev)) { Die "-MpvDev path does not exist: $MpvDev" }

  $dll = Get-ChildItem -Path $MpvDev -Filter 'libmpv-2.dll' -Recurse -ErrorAction SilentlyContinue |
         Select-Object -First 1
  if (-not $dll) { Die "libmpv-2.dll not found under $MpvDev" }
  $dir = $dll.DirectoryName

  $lib = Join-Path $dir 'mpv.lib'
  if (-not (Test-Path $lib)) {
    $def = Join-Path $dir 'mpv.def'
    if (-not (Test-Path $def)) {
      Die "neither mpv.lib nor mpv.def in $dir — the archive is not an mpv-dev build"
    }
    Log 'generating mpv.lib from mpv.def'
    Ensure-MsvcTools
    # /NAME matters: without it the import library would reference mpv.dll,
    # and the file that actually ships is libmpv-2.dll.
    & lib.exe /def:"$def" /name:libmpv-2.dll /out:"$lib" /machine:x64 | Out-Null
    if ($LASTEXITCODE -ne 0) { Die 'lib.exe failed to build mpv.lib' }
  }

  [pscustomobject]@{ Dir = $dir; Dll = $dll.FullName; Lib = $lib }
}

# lib.exe and link.exe come from a Developer prompt. If this is a plain
# PowerShell session, find the toolchain with vswhere and import its environment.
function Ensure-MsvcTools {
  if (Get-Command lib.exe -ErrorAction SilentlyContinue) { return }

  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  if (-not (Test-Path $vswhere)) { Die 'lib.exe not on PATH and vswhere.exe not found — run from a Developer PowerShell' }

  $vsPath = & $vswhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  if (-not $vsPath) { Die 'no Visual Studio C++ toolchain found' }

  $devCmd = Join-Path $vsPath 'Common7\Tools\VsDevCmd.bat'
  Log 'importing the MSVC environment'
  cmd /c "`"$devCmd`" -arch=amd64 -host_arch=amd64 >nul && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] }
  }
  if (-not (Get-Command lib.exe -ErrorAction SilentlyContinue)) { Die 'still no lib.exe after importing VsDevCmd' }
}

# --------------------------------------------------------------------- build ---

function Invoke-Build ($mpv) {
  if ($SkipBuild) { Log 'skipping cargo build'; return }

  # The dev-only path patch; say so plainly rather than deep in a resolve error.
  $manifest = Get-Content (Join-Path $Root 'Cargo.toml') -Raw
  if ($manifest -match '(?m)^\[patch\.crates-io\]' -and -not (Test-Path (Join-Path $Root '..\proton-sdk-rs'))) {
    Die 'Cargo.toml still carries [patch.crates-io] -> ../proton-sdk-rs, but that checkout is missing.'
  }

  # MSVC reads the library search path out of LIB.
  $env:LIB = "$($mpv.Dir);$env:LIB"

  Log "cargo build --release --target $Target"
  Push-Location $Root
  try {
    & cargo build --release --locked --target $Target --bin proton-stream --bin pstr
    if ($LASTEXITCODE -ne 0) { Die 'cargo build failed' }
  } finally { Pop-Location }
}

function Stage ($mpv) {
  Log 'staging into dist\windows'
  $rel = Join-Path $Root "target\$Target\release"
  foreach ($f in 'proton-stream.exe', 'pstr.exe') {
    if (-not (Test-Path (Join-Path $rel $f))) { Die "$f missing from $rel — drop -SkipBuild" }
  }

  New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
  Copy-Item (Join-Path $rel 'proton-stream.exe') $OutDir -Force
  Copy-Item (Join-Path $rel 'pstr.exe')          $OutDir -Force
  Copy-Item $mpv.Dll                             $OutDir -Force
  Copy-Item (Join-Path $Root 'README.md')        $OutDir -Force

  if ($SignCertThumbprint) {
    foreach ($f in 'proton-stream.exe', 'pstr.exe') {
      Invoke-Sign (Join-Path $OutDir $f)
    }
  }
}

function Invoke-Sign ($path) {
  Log "signing $(Split-Path -Leaf $path)"
  $cert = Get-ChildItem Cert:\CurrentUser\My, Cert:\LocalMachine\My |
          Where-Object Thumbprint -eq $SignCertThumbprint | Select-Object -First 1
  if (-not $cert) { Die "no certificate with thumbprint $SignCertThumbprint" }
  Set-AuthenticodeSignature -FilePath $path -Certificate $cert `
    -TimestampServer $TimestampUrl -HashAlgorithm SHA256 | Out-Null
}

# ----------------------------------------------------------------- portable ---

function New-Zip {
  $out = Join-Path $Dist "proton-stream-$Version-x86_64-windows.zip"
  Log "zip -> $(Split-Path -Leaf $out)"
  if (Test-Path $out) { Remove-Item $out }
  Compress-Archive -Path (Join-Path $OutDir '*') -DestinationPath $out
}

# ---------------------------------------------------------------------- msi ---

function Ensure-Wix {
  if (Get-Command wix -ErrorAction SilentlyContinue) { return }
  if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
    Die 'the .msi needs the WiX toolset: install the .NET SDK, then `dotnet tool install --global wix`'
  }
  Log 'installing the WiX toolset'
  & dotnet tool install --global wix | Out-Null
  $env:PATH = "$env:USERPROFILE\.dotnet\tools;$env:PATH"
  if (-not (Get-Command wix -ErrorAction SilentlyContinue)) { Die 'wix still not on PATH after install' }
}

function New-Msi ($mpv) {
  Ensure-Wix
  # WixUI_InstallDir lives in the UI extension, which is per-installation state
  # rather than per-project; adding it twice is a no-op.
  & wix extension add -g WixToolset.UI.wixext 2>&1 | Out-Null

  # The RTF the license dialog shows. Prefer the repository's own LICENSE, so
  # the installer cannot drift from it; fall back to the checked-in copy.
  $rtf = Join-Path $Pkg 'license.rtf'
  $repoLicense = Join-Path $Root 'LICENSE'
  if (Test-Path $repoLicense) {
    $rtf = Join-Path $Dist 'license.rtf'
    # RTF escapes are backslash, brace-open and brace-close; paragraphs are
    # \par. Done per line, because a lookbehind over already-rewritten line
    # endings is a good way to emit \par twice.
    $body = ((Get-Content $repoLicense) | ForEach-Object {
      ($_ -replace '\\', '\\') -replace '([{}])', '\$1'
    }) -join "\par`r`n"
    @"
{\rtf1\ansi\ansicpg1252\deff0\nouicompat{\fonttbl{\f0\fnil\fcharset0 Segoe UI;}}
\viewkind4\uc1\pard\f0\fs20
$body
}
"@ | Set-Content -Path $rtf -Encoding ASCII
  } else {
    Warn 'no LICENSE in the repo root; the installer shows packaging\windows\license.rtf'
  }

  $out = Join-Path $Dist "proton-stream-$Version-x64.msi"
  Log "msi -> $(Split-Path -Leaf $out)"

  # Every -d value is built into a string first: PowerShell does not evaluate a
  # parenthesised expression glued to the middle of a bare argument token, so
  # `-d MpvDll=(Join-Path ...)` would be passed through literally.
  $wxs     = Join-Path $Pkg 'proton-stream.wxs'
  $mpvDll  = Join-Path $OutDir (Split-Path -Leaf $mpv.Dll)
  $icon    = Join-Path $Pkg 'proton-stream.ico'
  $readme  = Join-Path $Root 'README.md'

  $wixArgs = @(
    'build', '-arch', 'x64', '-ext', 'WixToolset.UI.wixext', $wxs,
    '-d', "Version=$Version",
    '-d', "BinDir=$OutDir",
    '-d', "MpvDll=$mpvDll",
    '-d', "IconFile=$icon",
    '-d', "LicenseRtf=$rtf",
    '-d', "ReadMe=$readme",
    '-o', $out
  )
  & wix @wixArgs
  if ($LASTEXITCODE -ne 0) { Die 'wix build failed' }

  if ($SignCertThumbprint) { Invoke-Sign $out }
}

# --------------------------------------------------------------------- main ---

New-Item -ItemType Directory -Force -Path $Dist | Out-Null
$mpv = Resolve-MpvDev
Invoke-Build $mpv
Stage $mpv

foreach ($a in $Artifacts) {
  switch ($a) {
    'exe' { }          # Stage already produced dist\windows\proton-stream.exe
    'zip' { New-Zip }
    'msi' { New-Msi $mpv }
  }
}

Log 'done — dist\'
Get-ChildItem $Dist -File | ForEach-Object {
  '    {0,10:N0} KB  {1}' -f ($_.Length / 1KB), $_.Name
}
