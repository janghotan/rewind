#!/usr/bin/env bash
# Build a local RPM (and SRPM) from the current working tree.
#
# Usage:
#   packaging/build-rpm.sh [--with-vendor] [--nocheck]
#
# The spec expects a vendor tarball (Source1). With --with-vendor this script
# generates it from Cargo.lock using `cargo vendor`; otherwise an existing
# packaging/rewind-<version>-vendor.tar.xz is reused and generated if missing.
#
# --nocheck skips %check, which is useful for quick local packaging runs.
#
# Requirements: rpm-build, cargo, git.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
name="rewind-$version"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

with_vendor=0
nocheck=0
for arg in "$@"; do
    case "$arg" in
        --with-vendor) with_vendor=1 ;;
        --nocheck) nocheck=1 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

vendor_archive="$root/packaging/$name-vendor.tar.xz"
if [[ $with_vendor -eq 1 || ! -f $vendor_archive ]]; then
    echo "==> generating vendor tarball (requires network or a full cargo cache)"
    cargo vendor --locked --manifest-path "$root/Cargo.toml" "$work/vendor" >/dev/null
    tar -C "$work" -cJf "$vendor_archive" vendor
fi
cp "$vendor_archive" "$work/$name-vendor.tar.xz"

echo "==> creating source tarball"
git -C "$root" archive --format=tar.gz --prefix="$name/" \
    --output="$work/$name.tar.gz" HEAD

echo "==> running rpmbuild"
mkdir -p "$root/packaging/rpmbuild"/{SRPMS,RPMS}
rpmbuild -ba ${nocheck:+--nocheck} "$root/packaging/rewind.spec" \
    --define "_sourcedir $work" \
    --define "_srcrpmdir $root/packaging/rpmbuild/SRPMS" \
    --define "_rpmdir $root/packaging/rpmbuild/RPMS" \
    --define "_builddir $work/build" \
    --define "_topdir $work/topdir"

echo
echo "Artifacts:"
find "$root/packaging/rpmbuild" -type f \( -name '*.rpm' \) -print
