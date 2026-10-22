# Packaging

rewind ships as a single static binary plus three shell hook files and
generated completions. There are no service files, post-install scripts, or
runtime dependencies.

| Path | Purpose |
|---|---|
| `packaging/rewind.spec` | Fedora/RHEL RPM spec, used by COPR |
| `packaging/build-rpm.sh` | Local RPM/SRPM build helper |
| `packaging/build-deb.sh` | Local `.deb` or source-package build helper |
| `debian/` | Debian/Ubuntu packaging |
| `.github/workflows/release.yml` | Static musl release binaries |

## Versioning

All packaging derives its version from `Cargo.toml` and `debian/changelog`.
When releasing, update both and add a `CHANGELOG.md` entry:

```bash
# Cargo.toml: version = "0.2.0"
dch -v 0.2.0-1 "rewind 0.2.0"
```

## RPM (Fedora, RHEL, COPR)

The spec builds offline from a vendored crate tree. Generate
`packaging/rewind-<version>-vendor.tar.xz` with `cargo vendor`, then build:

```bash
# On Fedora:
sudo dnf install rpm-build rpmdevtools cargo rust gcc git
packaging/build-rpm.sh --with-vendor

# Artifacts:
#   packaging/rpmbuild/RPMS/<arch>/rewind-<version>-1.<dist>.<arch>.rpm
#   packaging/rpmbuild/SRPMS/rewind-<version>-1.<dist>.src.rpm
```

### Publishing to COPR

One-time setup:

```bash
sudo dnf install copr-cli
# API token from https://copr.fedorainfracloud.org/api/
copr-cli create rewind \
  --chroot fedora-44-x86_64 \
  --chroot fedora-44-aarch64 \
  --chroot fedora-45-x86_64 \
  --chroot fedora-45-aarch64 \
  --description "Universal undo for destructive shell commands" \
  --instructions "Enable with: sudo dnf copr enable janghotan/rewind"
```

Each release:

```bash
copr-cli build rewind packaging/rpmbuild/SRPMS/rewind-<version>-1.*.src.rpm
copr-cli list-builds rewind
```

COPR rebuilds the SRPM in each chroot with network access disabled; the
vendor tarball is therefore mandatory. To iterate without COPR, use `mock`
on Fedora (not available on Ubuntu):

```bash
sudo dnf install mock
mock -r fedora-44-x86_64 --rebuild packaging/rpmbuild/SRPMS/rewind-*.src.rpm
```

## APT repository (GitHub Pages)

The production Debian/Ubuntu channel is a signed flat APT repository served
from GitHub Pages:

- Site: <https://janghotan.github.io/apt/>
- Repository: <https://github.com/janghotan/apt> (`gh-pages` branch)
- Architectures: `amd64`, `arm64`
- Packages: statically linked musl binaries built by
  `packaging/build-static-deb.sh`, so they have no runtime dependencies

The signing key lives in the `APT_SIGNING_KEY` secret of this repository; the
`APT_DEPLOY_KEY` secret is a write-enabled deploy key for `janghotan/apt`.
Both are used by `.github/workflows/apt.yml`, which the release workflow calls
after publishing a GitHub Release.

To republish a release manually:

```bash
gh workflow run apt.yml -f tag=v0.1.0
```

To update the repository from a workstation:

```bash
export GNUPGHOME=/path/to/apt-signing-key
git clone --branch gh-pages git@github.com:janghotan/apt.git
cd apt
./publish.sh /path/to/rewind_<version>-1_amd64.deb /path/to/rewind_<version>-1_arm64.deb
git commit -am "apt: publish <version>"
git push
```

The archive key fingerprint is
`5E4B 6F8C F986 92B9 3FA0 930E 4B23 A539 82AF 1E48`. Back it up offline; losing
it forces every user to re-trust a new key.

## Debian and Ubuntu packages

```bash
sudo apt install devscripts debhelper fakeroot cargo rustc git
packaging/build-deb.sh            # binary .deb in packaging/deb/
packaging/build-deb.sh --source   # .dsc + .changes for a PPA
lintian packaging/deb/*.deb
```

The source build vendors all crates because Launchpad buildds have no
network access. The generated `rewind_<version>.orig.tar.gz` contains
`vendor/` and `.cargo/config.toml`, so the PPA build is fully offline.

### Publishing to a PPA

One-time setup:

1. Create a Launchpad account and import your GPG key.
2. Create a PPA at <https://launchpad.net/~janghotan/+activate-ppa>.
3. Install `dput` and configure the `ppa` target (the default works).

Each release:

```bash
packaging/build-deb.sh --source
cd packaging/deb
debsign rewind_<version>-1_source.changes   # if not using -k with dput
dput ppa:janghotan/rewind rewind_<version>-1_source.changes
```

The changelog distribution must match the PPA series (`noble`, `oracular`,
etc.). Update `debian/changelog` before building for a different series.

## Release checklist

1. Bump `Cargo.toml` and `debian/changelog`; update `CHANGELOG.md`.
2. `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
3. Commit, tag `v<version>`, and push the tag. CI publishes static binaries
   and a GitHub Release.
4. Build the SRPM and submit it to COPR.
5. Build the source package and upload it to the PPA.
6. Verify installation on a clean container:

   ```bash
   docker run --rm -it fedora:44 sh -c \
     'dnf -y copr enable janghotan/rewind && dnf -y install rewind && rewind --version'
   ```
