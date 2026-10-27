%global debug_package %{nil}

Name:           rewind
Version:        0.2.0
Release:        1%{?dist}
Summary:        Universal undo for destructive shell commands
License:        MIT
URL:            https://github.com/janghotan/rewind
Source0:        %{url}/archive/refs/tags/v%{version}/%{name}-%{version}.tar.gz
# Generated with `cargo vendor`; see packaging/README.md.
Source1:        %{name}-%{version}-vendor.tar.xz
BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  git
BuildRequires:  rust

%description
rewind sits between your shell and commands that destroy state. It stages the
pre-action state of rm, mv, cp, chmod, chown, git reset --hard, git clean,
git checkout, git restore, and git branch -D, runs the real command with unchanged output and
exit code, and keeps a local journal so the action can be reversed later.

Undo is resumable and works across commands: one "rewind undo" reverses the
most recent destructive operation, and "rewind undo N" reverses the last N.
A single static binary provides the journal, trash, retention, and
configuration commands; bash, zsh, and fish hooks make the wrapping invisible.

%prep
%autosetup -n %{name}-%{version}
tar -xJf %{SOURCE1}
mkdir -p .cargo
cat > .cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

%build
export CARGO_HOME=%{_builddir}/cargo-home
export CARGO_NET_OFFLINE=true
cargo build --release --locked

%check
export CARGO_HOME=%{_builddir}/cargo-home
export CARGO_NET_OFFLINE=true
cargo test --release --locked

%install
install -Dm755 target/release/rewind %{buildroot}%{_bindir}/rewind
install -Dm644 docs/rewind.1 %{buildroot}%{_mandir}/man1/rewind.1
install -Dm644 shell/bash.sh %{buildroot}%{_datadir}/rewind/bash.sh
install -Dm644 shell/zsh.sh %{buildroot}%{_datadir}/rewind/zsh.sh
install -Dm644 shell/fish.fish %{buildroot}%{_datadir}/rewind/fish.fish

install -d %{buildroot}%{_datadir}/bash-completion/completions
%{buildroot}%{_bindir}/rewind completions bash > %{buildroot}%{_datadir}/bash-completion/completions/rewind
install -d %{buildroot}%{_datadir}/zsh/site-functions
%{buildroot}%{_bindir}/rewind completions zsh > %{buildroot}%{_datadir}/zsh/site-functions/_rewind
install -d %{buildroot}%{_datadir}/fish/vendor_completions.d
%{buildroot}%{_bindir}/rewind completions fish > %{buildroot}%{_datadir}/fish/vendor_completions.d/rewind.fish

%files
%license LICENSE
%doc README.md CONTRIBUTING.md CHANGELOG.md
%{_bindir}/rewind
%{_mandir}/man1/rewind.1*
%dir %{_datadir}/rewind
%{_datadir}/rewind/bash.sh
%{_datadir}/rewind/zsh.sh
%{_datadir}/rewind/fish.fish
%{_datadir}/bash-completion/completions/rewind
%{_datadir}/zsh/site-functions/_rewind
%{_datadir}/fish/vendor_completions.d/rewind.fish

%changelog
* Tue Oct 27 2026 janghotan <janghotan@users.noreply.github.com> - 0.2.0-1
- New upstream release.
- Add `rewind inspect` command.
- Restore empty directories on `git clean` undo.
- Add undo support for `git branch -D`.

* Thu Oct 22 2026 janghotan <janghotan@users.noreply.github.com> - 0.1.1-1
- New upstream release.
- Add static Debian package tooling and signed APT repository automation.
- Refresh the English and Simplified Chinese documentation.

* Wed Oct 21 2026 janghotan <janghotan@users.noreply.github.com> - 0.1.0-1
- Initial package.
