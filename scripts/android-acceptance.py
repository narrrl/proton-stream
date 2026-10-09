#!/usr/bin/env python3
"""On-device acceptance for the Android client.

Driven by scripts/android-acceptance.sh. Every case states what it observes and
why it matters; a case that cannot run says so rather than passing quietly.

Cases carry one of three dispositions:

  normal   expected to pass; a failure fails the run
  xfail    a known-open bug in docs/BUGS.md. Failing is expected and does not
           fail the run; *passing* fails it, because the bug is fixed and the
           case should be promoted
  pending  the case is specified but not implemented. It never passes, and the
           summary reports the matrix as incomplete until it is gone
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

REPO_ROOT = Path(__file__).resolve().parent.parent

# The debug build carries an applicationId suffix, so the installed package and
# the activity's class name do not share a prefix. Keeping them apart matters:
# `am start -n <pkg>/.MainActivity` expands the leading dot against the *package*
# argument, which on a debug build names a class that does not exist.
APPLICATION_ID = "io.narl.protonstream"
MAIN_ACTIVITY_CLASS = "io.narl.protonstream.MainActivity"

# Resolved in main() from --package/--apk; the release build has no suffix.
PACKAGE = f"{APPLICATION_ID}.debug"

LIVE_TEST_CLASS = "io.narl.protonstream.live.LiveShareTest"

DEBUG_APK = REPO_ROOT / "android/app/build/outputs/apk/debug/app-debug.apk"
RELEASE_APK = REPO_ROOT / "android/app/build/outputs/apk/release/app-release.apk"


class CaseError(Exception):
    """A case failed on an assertion it makes about the device."""


class CaseSkipped(Exception):
    """A case cannot run here — missing configuration, wrong device."""


class CasePending(Exception):
    """A case is specified but not implemented."""


@dataclass
class Case:
    name: str
    what: str
    run: Callable[["Device", "Options"], None]
    xfail: str | None = None
    needs_share: bool = False


REGISTRY: list[Case] = []


def case(name: str, what: str, xfail: str | None = None, needs_share: bool = False):
    def register(fn):
        REGISTRY.append(Case(name=name, what=what, run=fn, xfail=xfail, needs_share=needs_share))
        return fn

    return register


# --------------------------------------------------------------------------
# device


class Device:
    def __init__(self, serial: str | None, timeout: int):
        self.serial = serial
        self.timeout = timeout
        self._base_apk: Path | None = None

    def _adb(self, *args: str) -> list[str]:
        prefix = ["adb"]
        if self.serial:
            prefix += ["-s", self.serial]
        return prefix + list(args)

    def run(self, *args: str, check: bool = True, timeout: int | None = None) -> str:
        proc = subprocess.run(
            self._adb(*args),
            capture_output=True,
            text=True,
            timeout=timeout or self.timeout,
        )
        if check and proc.returncode != 0:
            raise CaseError(
                f"adb {' '.join(args)} failed ({proc.returncode}): "
                f"{(proc.stderr or proc.stdout).strip()}"
            )
        return proc.stdout

    def shell(self, command: str, check: bool = True, timeout: int | None = None) -> str:
        return self.run("shell", command, check=check, timeout=timeout)

    def prop(self, name: str) -> str:
        return self.shell(f"getprop {name}").strip()

    @property
    def api_level(self) -> int:
        return int(self.prop("ro.build.version.sdk") or 0)

    @property
    def abi(self) -> str:
        return self.prop("ro.product.cpu.abi")

    def pid(self) -> int | None:
        out = self.shell(f"pidof {PACKAGE}", check=False).strip()
        return int(out.split()[0]) if out else None

    def is_installed(self) -> bool:
        return bool(self.shell(f"pm list packages {PACKAGE}", check=False).strip())

    def wake(self) -> None:
        # A dark or locked screen has no resumed activity, so every case that
        # starts the app would fail for a reason that has nothing to do with the
        # app. `dismiss-keyguard` clears a swipe lock; a PIN or pattern needs a
        # human, which is what `assert_unlocked` reports.
        self.shell("input keyevent KEYCODE_WAKEUP", check=False)
        self.shell("wm dismiss-keyguard", check=False)
        time.sleep(0.5)

    def keyguard_locked(self) -> bool:
        out = self.shell("dumpsys window", check=False)
        match = re.search(r"mDreamingLockscreen=(\w+)", out)
        return bool(match and match.group(1) == "true")

    def launch(self) -> None:
        self.wake()
        self.shell(f"am start -W -n {PACKAGE}/{MAIN_ACTIVITY_CLASS}")

    def force_stop(self) -> None:
        self.shell(f"am force-stop {PACKAGE}", check=False)

    def clear_logcat(self) -> None:
        self.run("logcat", "-c", check=False)

    def logcat_since_clear(self) -> str:
        return self.run("logcat", "-d", check=False)

    def wait_for_process(self, seconds: float = 15.0) -> int:
        deadline = time.time() + seconds
        while time.time() < deadline:
            pid = self.pid()
            if pid:
                return pid
            time.sleep(0.25)
        raise CaseError(f"{PACKAGE} did not start within {seconds:.0f}s")

    def pull_base_apk(self) -> Path:
        # Pulled from the device rather than read from build/outputs, so the
        # case reports on what is actually installed even under --no-install.
        if self._base_apk is None:
            paths = [
                line.removeprefix("package:").strip()
                for line in self.shell(f"pm path {PACKAGE}").splitlines()
                if line.startswith("package:")
            ]
            base = next((path for path in paths if path.endswith("base.apk")), None)
            if base is None:
                raise CaseError(f"pm path reported no base.apk for {PACKAGE}")
            local = Path(tempfile.mkdtemp(prefix="pstr-acceptance-")) / "base.apk"
            self.run("pull", base, str(local), timeout=600)
            if not local.is_file():
                raise CaseError(f"could not pull {base} off the device")
            self._base_apk = local
        return self._base_apk

    def resumed_activity(self) -> str:
        out = self.shell("dumpsys activity activities | grep -E 'mResumedActivity|topResumedActivity'", check=False)
        return out.strip()


def assert_no_crash(device: Device) -> None:
    # Every check here is scoped to this app's own process. logcat is
    # system-wide, and a case that changes global state provokes crashes in
    # unrelated software: `offline-launch` toggles airplane mode, which reliably
    # kills Google Play services with a FATAL EXCEPTION that has nothing to do
    # with us. Attributing that to the app under test makes the suite lie.
    lines = device.logcat_since_clear().splitlines()
    owner = re.compile(r"\bProcess:\s*" + re.escape(PACKAGE) + r"\b")
    for index, line in enumerate(lines):
        if re.search(r"\bANR in " + re.escape(PACKAGE), line):
            raise CaseError(f"ANR in logcat:\n" + "\n".join(lines[index : index + 20]))
        if re.search(r"FATAL EXCEPTION", line, re.I):
            # The runtime prints `Process: <package>, PID: <n>` on the line
            # right after the FATAL EXCEPTION header.
            if any(owner.search(nxt) for nxt in lines[index : index + 4]):
                raise CaseError("crash in logcat:\n" + "\n".join(lines[index : index + 30]))
        if "Fatal signal" in line:
            # A native crash names the process in the tombstone header a few
            # lines below, as `>>> <package> <<<`.
            window = lines[index : index + 12]
            if any(f">>> {PACKAGE} <<<" in nxt for nxt in window):
                raise CaseError("native crash in logcat:\n" + "\n".join(window))


# --------------------------------------------------------------------------
# cases


@case("install", "the app is installed and its version is the workspace version")
def _install(device: Device, options: "Options") -> None:
    if not device.is_installed():
        raise CaseError(f"{PACKAGE} is not installed; run without --no-install")
    dumped = device.shell(f"dumpsys package {PACKAGE} | grep versionName")
    match = re.search(r"versionName=([^\s]+)", dumped)
    if not match:
        raise CaseError("dumpsys reported no versionName")
    installed = match.group(1).removesuffix("-debug")
    cargo = (REPO_ROOT / "Cargo.toml").read_text()
    expected = re.search(r'(?m)^version\s*=\s*"([^"]+)"', cargo)
    if not expected:
        raise CaseError("no workspace version in Cargo.toml")
    if installed != expected.group(1):
        raise CaseError(
            f"installed {installed} but the workspace is {expected.group(1)}; "
            "the APK is stale — rebuild before trusting this run"
        )


@case("native-libraries", "every native library the app dlopens is packaged for this ABI")
def _native_libraries(device: Device, options: "Options") -> None:
    # The three that must be there: the Rust bridge, the GPL libmpv, and the
    # JNI/EGL adapter between them. A missing one is a packaging fault that only
    # shows up when the viewer presses play.
    #
    # The APK is the authority, not the on-device `lib/` directory. With
    # `useLegacyPackaging = false` the installer never extracts anything — the
    # libraries are mapped straight out of the (uncompressed, page-aligned) APK,
    # so that directory is legitimately empty on a healthy install. Reading
    # /proc/<pid>/maps would be the direct runtime proof, but it is unreadable
    # for another app's process without root.
    apk = device.pull_base_apk()
    try:
        with zipfile.ZipFile(apk) as archive:
            entries = archive.namelist()
    except zipfile.BadZipFile as error:
        raise CaseError(f"the installed APK is not readable as a zip: {error}") from None

    abis = sorted({name.split("/")[1] for name in entries if name.startswith("lib/") and "/" in name[4:]})
    if device.abi not in abis:
        raise CaseError(
            f"the device is {device.abi} but the APK packages only {', '.join(abis) or '(no ABI)'}"
        )
    packaged = {name.rsplit("/", 1)[1] for name in entries if name.startswith(f"lib/{device.abi}/")}
    missing = [
        name
        for name in ("libpstr_android.so", "libmpv.so", "libpstr_mpv.so")
        if name not in packaged
    ]
    if missing:
        raise CaseError(
            f"not packaged for {device.abi}: {', '.join(missing)}\n"
            f"saw: {', '.join(sorted(packaged)) or '(nothing)'}"
        )


@case("manifest-hardening", "backup stays closed and no unexpected component is exported")
def _manifest_hardening(device: Device, options: "Options") -> None:
    dumped = device.shell(f"dumpsys package {PACKAGE}")
    flags = re.search(r"flags=\[([^\]]*)\]", dumped)
    if flags and "ALLOW_BACKUP" in flags.group(1):
        raise CaseError(
            "ALLOW_BACKUP is set: share fragments and link passwords would leave the device"
        )
    # Only MainActivity may be exported. Both services are internal, and anything
    # that can reach them inherits the daemon's authenticated session.
    exported = re.findall(r"^\s+(\S+/\S+)\s+filter", dumped, re.M)
    unexpected = [name for name in exported if "MainActivity" not in name]
    if unexpected:
        raise CaseError(f"unexpectedly exported components: {', '.join(sorted(set(unexpected)))}")


@case("cold-start", "a cold launch reaches a resumed activity without a crash")
def _cold_start(device: Device, options: "Options") -> None:
    device.force_stop()
    device.clear_logcat()
    device.launch()
    device.wait_for_process()
    time.sleep(2.0)
    assert_no_crash(device)
    resumed = device.resumed_activity()
    if APPLICATION_ID not in resumed:
        if device.keyguard_locked():
            raise CaseSkipped("the device is locked and needs a PIN; unlock it and run again")
        raise CaseError(f"no resumed activity for the app: {resumed or '(dumpsys reported none)'}")


@case("rotation", "rotating does not recreate the process or drop the activity")
def _rotation(device: Device, options: "Options") -> None:
    device.force_stop()
    device.clear_logcat()
    device.launch()
    before = device.wait_for_process()
    try:
        device.shell("settings put system accelerometer_rotation 0")
        for rotation in ("1", "0"):
            device.shell(f"settings put system user_rotation {rotation}")
            time.sleep(1.5)
        after = device.pid()
        if after != before:
            raise CaseError(f"the process was recreated by a rotation ({before} -> {after})")
        assert_no_crash(device)
    finally:
        device.shell("settings put system user_rotation 0", check=False)
        device.shell("settings put system accelerometer_rotation 1", check=False)


@case("tablet-layout", "a tablet-sized window lays out and survives a resize")
def _tablet_layout(device: Device, options: "Options") -> None:
    original_size = device.shell("wm size").strip()
    original_density = device.shell("wm density").strip()
    try:
        device.force_stop()
        device.clear_logcat()
        device.shell("wm size 1600x2560")
        device.shell("wm density 240")
        device.launch()
        device.wait_for_process()
        time.sleep(2.5)
        assert_no_crash(device)
    finally:
        device.shell("wm size reset", check=False)
        device.shell("wm density reset", check=False)
    _ = (original_size, original_density)


@case(
    "process-recreation",
    "state survives the process being killed under the app",
)
def _process_recreation(device: Device, options: "Options") -> None:
    # Was an xfail against B35, promoted when B35 landed: the navigation
    # destination and the open player are `rememberSaveable` keys now, so there
    # is saved instance state to come back to.
    device.force_stop()
    device.launch()
    device.wait_for_process()
    time.sleep(2.0)
    device.shell(f"am broadcast -a android.intent.action.MAIN -p {PACKAGE}", check=False)
    saved = device.shell(
        f"dumpsys activity {PACKAGE} | grep -c 'mSavedInstanceState'", check=False
    ).strip()
    device.clear_logcat()
    device.shell(f"am kill {PACKAGE}")
    time.sleep(1.0)
    device.launch()
    device.wait_for_process()
    time.sleep(2.0)
    assert_no_crash(device)
    if saved in ("", "0"):
        raise CaseError(
            "no saved instance state was recorded, so nothing can be restored "
            "after a process kill (B35)"
        )


@case("offline-launch", "the app starts and stays up with no network")
def _offline_launch(device: Device, options: "Options") -> None:
    device.force_stop()
    device.shell("cmd connectivity airplane-mode enable", check=False)
    time.sleep(2.0)
    try:
        device.clear_logcat()
        device.launch()
        device.wait_for_process()
        time.sleep(3.0)
        assert_no_crash(device)
    finally:
        device.shell("cmd connectivity airplane-mode disable", check=False)
        time.sleep(2.0)


def jdk17_home() -> str | None:
    candidates = [os.environ.get("JAVA_HOME", "")]
    candidates += [
        "/usr/lib/jvm/java-17-openjdk",
        "/usr/lib/jvm/java-17-openjdk-amd64",
        "/usr/lib/jvm/temurin-17-jdk",
        "/Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home",
    ]
    for home in candidates:
        java = Path(home) / "bin/java" if home else None
        if java is None or not java.is_file():
            continue
        try:
            out = subprocess.run(
                [str(java), "-version"], capture_output=True, text=True, timeout=30
            )
        except (OSError, subprocess.SubprocessError):
            continue
        first = (out.stderr + out.stdout).splitlines()[:1]
        if first and re.search(r'version "17[.\"]', first[0]):
            return home
    return None


def android_sdk_home() -> str | None:
    adb = shutil.which("adb")
    if adb:
        # <sdk>/platform-tools/adb
        sdk = Path(adb).resolve().parent.parent
        if (sdk / "platform-tools").is_dir():
            return str(sdk)
    for candidate in ("/opt/android-sdk", str(Path.home() / "Android/Sdk")):
        if Path(candidate, "platform-tools").is_dir():
            return candidate
    return None


def redact(text: str, share: "dict[str, str] | None") -> str:
    for value in (share or {}).values():
        if value:
            text = text.replace(value, "<redacted>")
    return text


def run_instrumentation(
    options: "Options",
    only: str | None = None,
    exclude: str | None = None,
    share: "dict[str, str] | None" = None,
) -> None:
    gradlew = REPO_ROOT / "android/gradlew"
    gradle = [str(gradlew)] if gradlew.is_file() and os.access(gradlew, os.X_OK) else None
    if gradle is None:
        if not shutil.which("gradle"):
            raise CaseSkipped("no android/gradlew and no gradle on PATH")
        gradle = ["gradle"]
    task = "connectedReleaseAndroidTest" if options.test_release else "connectedDebugAndroidTest"
    env = dict(os.environ)
    # `testBuildType` is read from this variable in build.gradle.kts, so without
    # it Gradle configures the debug variant and the release task simply does
    # not exist.
    env["ANDROID_TEST_BUILD_TYPE"] = "release" if options.test_release else "debug"
    # Same reason build-android.sh looks for it: on a newer JVM the Kotlin
    # compiler throws `IllegalArgumentException: 26.0.2` out of an IntelliJ
    # version parser, which reads as a broken build rather than a wrong JDK.
    java_home = jdk17_home()
    if java_home is None:
        raise CaseSkipped("no JDK 17 found; set JAVA_HOME (see docs/ANDROID.md)")
    env["JAVA_HOME"] = java_home
    # There is no android/local.properties in the repo, so Gradle finds the SDK
    # only through the environment. adb is on PATH here by definition, and it
    # lives in the SDK, so its location is the most reliable hint we have.
    if not env.get("ANDROID_HOME") and not env.get("ANDROID_SDK_ROOT"):
        sdk = android_sdk_home()
        if sdk is None:
            raise CaseSkipped("no Android SDK found; set ANDROID_HOME (see docs/ANDROID.md)")
        env["ANDROID_HOME"] = sdk
        env["ANDROID_SDK_ROOT"] = sdk
    arguments = list(gradle) + ["--no-daemon", task]
    if only:
        arguments.append(f"-Pandroid.testInstrumentationRunnerArguments.class={only}")
    if exclude:
        arguments.append(f"-Pandroid.testInstrumentationRunnerArguments.notClass={exclude}")
    for name, value in (share or {}).items():
        # Passed as a Gradle property rather than written anywhere: a share link
        # and its password are credentials, and this keeps them out of the repo,
        # the reports and the build directory. They are still visible in this
        # machine's process list for the duration of the run.
        arguments.append(f"-Pandroid.testInstrumentationRunnerArguments.{name}={value}")
    proc = subprocess.run(
        arguments,
        cwd=REPO_ROOT / "android",
        capture_output=True,
        text=True,
        timeout=max(options.timeout, 1800),
        env=env,
    )
    if proc.returncode != 0:
        tail = "\n".join(redact((proc.stdout + proc.stderr), share).splitlines()[-40:])
        raise CaseError(f"{task} failed:\n{tail}")
    if only and "Starting 0 tests" in proc.stdout:
        raise CaseError(f"no test matched {only}")


@case("instrumentation", "the on-device test suite passes")
def _instrumentation(device: Device, options: "Options") -> None:
    # The live class is excluded here and run by the cases below, so a missing
    # share cannot make this case look like it covered them.
    run_instrumentation(options, exclude=LIVE_TEST_CLASS)


def live_arguments(options: "Options") -> dict[str, str]:
    share = {"pstr.shareUrl": options.share_url or ""}
    if options.share_password:
        share["pstr.sharePassword"] = options.share_password
    return share



def _pending(reason: str):
    def run(device: Device, options: "Options") -> None:
        raise CasePending(reason)

    return run


# One run of the live class serves every live case. Running them separately
# would be cleaner in principle, but each run re-adds the share and re-crawls it
# from an empty catalog — six minutes apiece against a real share — and the
# crawl is not what any of these cases is testing.
LIVE_RUN: "dict[str, tuple[str, str]] | None" = None


def live_results(options: "Options") -> "dict[str, tuple[str, str]]":
    global LIVE_RUN
    if LIVE_RUN is not None:
        return LIVE_RUN
    results_dir = REPO_ROOT / "android/app/build/outputs/androidTest-results/connected" / (
        "release" if options.test_release else "debug"
    )
    for stale in results_dir.glob("*.xml"):
        stale.unlink()
    failure: CaseError | None = None
    try:
        run_instrumentation(options, only=LIVE_TEST_CLASS, share=live_arguments(options))
    except CaseError as error:
        # Individual methods are reported per case below; this only matters if
        # nothing ran at all.
        failure = error
    parsed: dict[str, tuple[str, str]] = {}
    for report in results_dir.glob("*.xml"):
        for testcase in ET.parse(report).getroot().iter("testcase"):
            name = testcase.get("name") or ""
            problem = next(iter(testcase.iter("failure")), None)
            if problem is not None:
                parsed[name] = ("failed", (problem.text or "").strip())
            elif next(iter(testcase.iter("skipped")), None) is not None:
                parsed[name] = ("skipped", "the test skipped itself by assumption")
            else:
                parsed[name] = ("passed", "")
    if not parsed:
        raise failure or CaseError("the live suite produced no results")
    LIVE_RUN = parsed
    return parsed


def _live(method: str):
    """Reads one method out of the shared live run; the share is never stored."""

    def run(device: Device, options: "Options") -> None:
        results = live_results(options)
        status, detail = results.get(method, ("missing", ""))
        if status == "failed":
            raise CaseError(detail or f"{method} failed")
        if status == "skipped":
            raise CaseSkipped(detail)
        if status == "missing":
            raise CaseError(f"{method} did not run; the live class may have been renamed")

    return run


for _name, _what, _method in (
    (
        "playback",
        "an episode decodes through libmpv from the encrypted stream",
        "libmpvDecodesTheDecryptedStream",
    ),
    (
        "catalog",
        "the share link resolves into a catalog of playable files",
        "theShareResolvesIntoAPlayableCatalog",
    ),
    (
        "stream-open",
        "an episode opens as a sized, seekable revision",
        "openingAnEpisodeYieldsASeekableRevision",
    ),
    (
        "download-cancel-resume",
        "a cancelled download leaves a resumable .part and resumes from it",
        "aCancelledDownloadLeavesAResumablePartFile",
    ),
    (
        "watch-state",
        "a resume position survives reopening the catalog",
        "aResumePositionSurvivesReopeningTheCatalog",
    ),
):
    REGISTRY.append(Case(name=_name, what=_what, run=_live(_method), needs_share=True))


# --------------------------------------------------------------------------
# runner


@dataclass
class Options:
    timeout: int = 120
    fail_fast: bool = False
    test_release: bool = False
    share_url: str | None = None
    share_password: str | None = None


@dataclass
class Result:
    name: str
    status: str  # passed | failed | skipped | xfailed | xpassed | pending
    detail: str = ""
    seconds: float = 0.0


@dataclass
class Run:
    results: list[Result] = field(default_factory=list)

    def count(self, status: str) -> int:
        return sum(1 for result in self.results if result.status == status)


def select(names: list[Case], only: str | None) -> list[Case]:
    if not only:
        return names
    wanted = [part.strip() for part in only.split(",") if part.strip()]
    return [case for case in names if any(part in case.name for part in wanted)]


def run_cases(device: Device, cases: list[Case], options: Options) -> Run:
    run = Run()
    for entry in cases:
        started = time.time()
        status, detail = "passed", ""
        try:
            # The share check comes second on purpose: an unimplemented case is
            # pending whether or not a share is configured, and reporting it as
            # "skipped" would let a missing environment variable disguise a hole
            # in the matrix.
            entry.run(device, options)
            if entry.needs_share and not options.share_url:
                raise CaseSkipped("PSTR_ACCEPTANCE_SHARE_URL is not set")
            if entry.xfail:
                status = "xpassed"
                detail = f"{entry.xfail} looks fixed — promote this case and close it"
        except CasePending as pending:
            status, detail = "pending", str(pending)
        except CaseSkipped as skipped:
            status, detail = "skipped", str(skipped)
        except (CaseError, subprocess.TimeoutExpired) as failure:
            if entry.xfail:
                status, detail = "xfailed", f"{entry.xfail}: {failure}"
            else:
                status, detail = "failed", str(failure)
        seconds = time.time() - started
        run.results.append(Result(entry.name, status, detail, seconds))
        mark = {
            "passed": "ok",
            "failed": "FAIL",
            "skipped": "skip",
            "xfailed": "xfail",
            "xpassed": "XPASS",
            "pending": "pending",
        }[status]
        print(f"[{mark:>7}] {entry.name} ({seconds:.1f}s)")
        if detail:
            for line in detail.splitlines():
                print(f"          {line}")
        if options.fail_fast and status in ("failed", "xpassed"):
            break
    return run


def write_json(run: Run, path: Path) -> None:
    path.write_text(
        json.dumps(
            {
                "results": [
                    {
                        "name": result.name,
                        "status": result.status,
                        "detail": result.detail,
                        "seconds": round(result.seconds, 3),
                    }
                    for result in run.results
                ]
            },
            indent=2,
        )
    )


def write_junit(run: Run, path: Path) -> None:
    suite = ET.Element(
        "testsuite",
        name="android-acceptance",
        tests=str(len(run.results)),
        failures=str(run.count("failed") + run.count("xpassed")),
        skipped=str(run.count("skipped") + run.count("pending")),
    )
    for result in run.results:
        node = ET.SubElement(
            suite, "testcase", name=result.name, time=f"{result.seconds:.3f}"
        )
        if result.status in ("failed", "xpassed"):
            ET.SubElement(node, "failure", message=result.detail or result.status)
        elif result.status in ("skipped", "pending", "xfailed"):
            ET.SubElement(node, "skipped", message=result.detail or result.status)
    ET.ElementTree(suite).write(path, encoding="utf-8", xml_declaration=True)


def attached_serials() -> list[str]:
    out = subprocess.run(["adb", "devices"], capture_output=True, text=True).stdout
    return [
        line.split()[0]
        for line in out.splitlines()[1:]
        if line.strip() and line.split()[-1] == "device"
    ]


def main() -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--serial")
    parser.add_argument("--apk")
    parser.add_argument(
        "--package",
        help="installed applicationId; inferred from the APK's build type when omitted",
    )
    parser.add_argument(
        "--release",
        action="store_true",
        help="test the minified release APK — the build that actually ships",
    )
    parser.add_argument("--no-install", action="store_true")
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--fail-fast", action="store_true")
    parser.add_argument("--timeout", type=int, default=120)
    parser.add_argument("--report-json")
    parser.add_argument("--report-junit")
    parser.add_argument("--keep-going-on-crash", action="store_true")
    args = parser.parse_args()

    release = args.release or os.environ.get("ANDROID_TEST_BUILD_TYPE") == "release"
    apk = Path(args.apk) if args.apk else (RELEASE_APK if release else DEBUG_APK)
    global PACKAGE
    if args.package:
        PACKAGE = args.package
    elif "debug" in apk.name or "debug" in apk.parent.name:
        PACKAGE = f"{APPLICATION_ID}.debug"
    else:
        PACKAGE = APPLICATION_ID

    cases = select(REGISTRY, os.environ.get("PSTR_ACCEPTANCE_ONLY"))
    if args.list:
        width = max(len(entry.name) for entry in cases) if cases else 0
        for entry in cases:
            tags = []
            if entry.xfail:
                tags.append(f"xfail:{entry.xfail}")
            if entry.needs_share:
                tags.append("live")
            suffix = f"  [{', '.join(tags)}]" if tags else ""
            print(f"{entry.name:<{width}}  {entry.what}{suffix}")
        return 0
    if not cases:
        print("no case matched PSTR_ACCEPTANCE_ONLY", file=sys.stderr)
        return 2

    serials = attached_serials()
    serial = args.serial
    if serial is None:
        if len(serials) != 1:
            print(
                f"expected exactly one attached device, found {len(serials)}"
                + (f": {', '.join(serials)}" if serials else ""),
                file=sys.stderr,
            )
            return 2
        serial = serials[0]

    device = Device(serial, args.timeout)
    if device.api_level < 31:
        print(
            f"the app targets Android 12 (API 31) and newer; this device is API {device.api_level}",
            file=sys.stderr,
        )
        return 2

    if not args.no_install:
        if not apk.is_file():
            build = "release" if release else "debug"
            print(
                f"no APK at {apk}; run `bash scripts/build-android.sh {build}` first",
                file=sys.stderr,
            )
            return 2
        print(f"installing {apk.name} as {PACKAGE} on {serial} ({device.abi}, API {device.api_level})")
        device.run("install", "-r", "-g", str(apk), timeout=600)

    options = Options(
        timeout=args.timeout,
        fail_fast=args.fail_fast,
        test_release=release,
        share_url=os.environ.get("PSTR_ACCEPTANCE_SHARE_URL"),
        share_password=os.environ.get("PSTR_ACCEPTANCE_SHARE_PASSWORD"),
    )
    run = run_cases(device, cases, options)

    if args.report_json:
        write_json(run, Path(args.report_json))
    if args.report_junit:
        write_junit(run, Path(args.report_junit))

    print()
    print(
        "  ".join(
            f"{status}={run.count(status)}"
            for status in ("passed", "failed", "xfailed", "xpassed", "skipped", "pending")
        )
    )
    if run.count("pending"):
        print(
            f"the matrix docs/ANDROID.md requires is incomplete: "
            f"{run.count('pending')} case(s) are specified but not implemented"
        )
    if run.count("xpassed"):
        print("a case marked xfail passed — the bug it tracks is fixed; promote the case")

    return 1 if run.count("failed") or run.count("xpassed") else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
