#!/usr/bin/env bash
# Build Debian packages from a vendored copy of the current tree.
#
# Usage:
#   packaging/build-deb.sh [--source] [--no-vendor]
#
# Launchpad has no network access during builds, so the source tarball that is
# uploaded to a PPA must contain every crate. This script creates vendor/ and
# .cargo/config.toml in a temporary copy of the tree, then runs dpkg-buildpackage
# against it. Artifacts are copied to packaging/deb/.
#
#   --source      build a signed-ready source package (.dsc + .changes) for dput
#   --no-vendor   skip vendoring (local builds with a warm cargo cache)
#
# Requirements: cargo, dpkg-dev, debhelper, fakeroot, git.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
name="rewind-$version"
debversion=$(dpkg-parsechangelog -l "$root/debian/changelog" -S Version)
upstream=${debversion%-*}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

vendor=1
source_only=0
for arg in "$@"; do
    case "$arg" in
        --source) source_only=1 ;;
        --no-vendor) vendor=0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

echo "==> exporting source tree"
git -C "$root" archive --format=tar --prefix="$name/" HEAD | tar -x -C "$work"
cd "$work/$name"

if [[ $vendor -eq 1 ]]; then
    echo "==> vendoring dependencies"
    cargo vendor --locked vendor >/dev/null
    mkdir -p .cargo
    cat > .cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF
fi

# The orig tarball must sit next to the unpacked source directory and carry
# the Debian upstream version (without the revision).
orig="$work/rewind_${upstream}.orig.tar.gz"
tar -C "$work" -czf "$orig" "$name"

if [[ $source_only -eq 1 ]]; then
    echo "==> building source package"
    # -d skips build-dependency installation; -us/-uc leave signing to dput
    # (or to `debsign`).
    dpkg-buildpackage -us -uc -d -S
else
    echo "==> building binary package"
    dpkg-buildpackage -us -uc -d -b
fi

echo "==> collecting artifacts"
mkdir -p "$root/packaging/deb"
for pattern in ../*.deb ../*.dsc ../*.changes ../*.buildinfo ../*.tar.*; do
    for file in $pattern; do
        [[ -e $file ]] && cp "$file" "$root/packaging/deb/"
    done
done

find "$root/packaging/deb" -maxdepth 1 -type f -print
