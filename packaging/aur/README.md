# AUR packages

The source of truth for the three AUR packages of this project. Each
subdirectory is the full content of one AUR git repository (`pkgbase` = the
directory name):

| Directory | What it does | AUR repository |
| --- | --- | --- |
| `proton-stream` | Builds the latest tagged release from source | `ssh://aur@aur.archlinux.org/proton-stream.git` |
| `proton-stream-git` | Builds `main` from git | `ssh://aur@aur.archlinux.org/proton-stream-git.git` |
| `proton-stream-bin` | Installs the binaries from the GitHub release tarball | `ssh://aur@aur.archlinux.org/proton-stream-bin.git` |

The three packages conflict with each other. `packaging/PKGBUILD` is a
different thing: it builds the checkout it sits in, for `makepkg -fi` during
development, and is never published.

Each directory tracks only `PKGBUILD`, `.SRCINFO`, `.gitignore` and
`LICENSE` (the 0BSD license for the packaging sources, per
[RFC 40](https://rfc.archlinux.page/0040-license-package-sources/)). The
`.gitignore` ignores everything else, so downloaded sources, `src/`, `pkg/`
and built packages never reach the AUR.

## Releasing a new version

`.github/workflows/aur.yml` does this automatically:

- **On a release tag**, the release workflow calls it once the GitHub release is
  published. It runs `./update.sh <version>`, pushes all three packages with
  `./push.sh`, and commits the updated recipes back to `main` as
  `chore(aur): update packages to <version>`.
- **On a push to `main` that changes `proton-stream-git/`**, it regenerates
  that package's `.SRCINFO` and pushes only the -git package.
- **By hand**, run the workflow from the Actions tab. Give a version to publish
  that release, or leave it empty to publish only the -git package.

It needs the `AUR_SSH_PRIVATE_KEY` repository secret: the private key whose
public half is registered on the AUR account.

The same by hand:

1. Tag and publish the upstream release, so its assets exist.
2. Run `./update.sh <version>`. It rewrites `pkgver`, resets `pkgrel=1`,
   refreshes the checksums and regenerates every `.SRCINFO`.
3. Build and inspect a package in a clean chroot:
   `cd proton-stream && pkgctl build --clean` (needs `devtools`).
4. Commit here, then publish with `./push.sh` (or `./push.sh proton-stream-bin`
   for a single package).

`proton-stream-git` carries a `pkgver()` function, so its `pkgver` is computed at
build time; do not push a commit that only bumps it. That is why new commits
on `main` do not trigger an AUR push.

## Requirements for pushing

An SSH key registered on the AUR account, and an entry in `~/.ssh/config`:

```
Host aur.archlinux.org
  IdentityFile ~/.ssh/aur
  User aur
```

`push.sh` makes a scratch clone of each AUR repository, copies the tracked
files over it, commits and pushes `master`. The first push of a package
creates it on the AUR.
