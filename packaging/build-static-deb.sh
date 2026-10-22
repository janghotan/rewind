#!/usr/bin/env bash
# Build a self-contained .deb from a static musl release binary.
#
# Usage:
#   packaging/build-static-deb.sh <binary> <amd64|arm64> [output-dir]
#
# The binary must be a statically linked Linux build for the given
# architecture. Completions are generated with REWIND_HOST_BIN when set (useful
# when cross-packaging arm64 on an x86_64 host); otherwise the target binary is
# executed, which only works for a native package.
#
# The result has no runtime dependencies beyond the kernel, so it installs on
# any Debian/Ubuntu release with a compatible dpkg.
set -euo pipefail
umask 022

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
binary=${1:?usage: build-static-deb.sh <binary> <amd64|arm64> [output-dir]}
arch=${2:?usage: build-static-deb.sh <binary> <amd64|arm64> [output-dir]}
outdir=${3:-$root/packaging/deb}

case "$arch" in
    amd64 | arm64) ;;
    *) echo "unsupported architecture: $arch (use amd64 or arm64)" >&2; exit 2 ;;
esac

[[ -x $binary ]] || { echo "binary is not executable: $binary" >&2; exit 2; }

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
host_bin=${REWIND_HOST_BIN:-$root/target/release/rewind}
if [[ ! -x $host_bin ]]; then
    host_bin=$binary
fi

stage=$(mktemp -d)
chmod 755 "$stage"
trap 'rm -rf "$stage"' EXIT

install -Dm755 "$binary" "$stage/usr/bin/rewind"
install -Dm644 "$root/shell/bash.sh" "$stage/usr/share/rewind/bash.sh"
install -Dm644 "$root/shell/zsh.sh" "$stage/usr/share/rewind/zsh.sh"
install -Dm644 "$root/shell/fish.fish" "$stage/usr/share/rewind/fish.fish"

install -d "$stage/usr/share/man/man1"
gzip -9cn "$root/docs/rewind.1" > "$stage/usr/share/man/man1/rewind.1.gz"

install -d "$stage/usr/share/bash-completion/completions"
"$host_bin" completions bash > "$stage/usr/share/bash-completion/completions/rewind"
install -d "$stage/usr/share/zsh/vendor-completions"
"$host_bin" completions zsh > "$stage/usr/share/zsh/vendor-completions/_rewind"
install -d "$stage/usr/share/fish/vendor_completions.d"
"$host_bin" completions fish > "$stage/usr/share/fish/vendor_completions.d/rewind.fish"

docdir="$stage/usr/share/doc/rewind"
install -d "$docdir"
gzip -9cn "$root/README.md" > "$docdir/README.md.gz"
gzip -9cn "$root/CHANGELOG.md" > "$docdir/CHANGELOG.md.gz"
gzip -9cn "$root/CONTRIBUTING.md" > "$docdir/CONTRIBUTING.md.gz"
install -Dm644 "$root/debian/copyright" "$docdir/copyright"

size=$(du -sk "$stage/usr" | cut -f1)
install -d "$stage/DEBIAN"
cat > "$stage/DEBIAN/control" <<EOF
Package: rewind
Version: ${version}-1
Architecture: ${arch}
Maintainer: janghotan <janghotan@users.noreply.github.com>
Installed-Size: ${size}
Section: utils
Priority: optional
Homepage: https://github.com/janghotan/rewind
Description: universal undo for destructive shell commands
 rewind sits between your shell and commands that destroy state. It stages
 the pre-action state of rm, mv, cp, chmod, chown, git reset --hard, git
 clean, git checkout, and git restore, runs the real command with unchanged
 output and exit code, and keeps a local journal so the action can be reversed
 later.
 .
 Undo is resumable and works across commands: one "rewind undo" reverses the
 most recent destructive operation, and "rewind undo N" reverses the last N.
 A single static binary provides the journal, trash, retention, and
 configuration commands; bash, zsh, and fish hooks make the wrapping invisible.
EOF

mkdir -p "$outdir"
package="$outdir/rewind_${version}-1_${arch}.deb"
dpkg-deb --build --root-owner-group -Zxz "$stage" "$package" >/dev/null
echo "$package"
