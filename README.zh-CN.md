<div align="center">
  <img src="assets/icon.svg" width="96" alt="rewind">
  <h1>rewind</h1>
  <p><strong>为破坏性 shell 命令提供通用撤销。</strong></p>
  <p><code>rm</code> · <code>mv</code> · <code>cp</code> · <code>chmod</code> · <code>chown</code> · 破坏性 <code>git</code></p>

  [![CI](https://img.shields.io/github/actions/workflow/status/janghotan/rewind/ci.yml?style=flat-square&label=CI)](https://github.com/janghotan/rewind/actions/workflows/ci.yml)
  [![Release](https://img.shields.io/github/v/release/janghotan/rewind?style=flat-square&color=34d399)](https://github.com/janghotan/rewind/releases/latest)
  [![APT repository](https://img.shields.io/badge/APT-repository-34d399?style=flat-square)](https://janghotan.github.io/apt/)
  [![COPR](https://img.shields.io/badge/COPR-janghotan%2Frewind-51a2da?style=flat-square)](https://copr.fedorainfracloud.org/coprs/janghotan/rewind/)
  [![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)

  <p><a href="README.md">English</a> · <strong>简体中文</strong></p>
  <p><a href="#快速开始">快速开始</a> · <a href="#用法">用法</a> · <a href="#覆盖范围">覆盖范围</a> · <a href="#工作原理">工作原理</a> · <a href="#配置">配置</a></p>
</div>

`rewind` 位于你的 shell 和会破坏状态的命令之间。它精确暂存即将发生变化的内容，用原本的输出和退出码运行真正的命令，并在本地日志中记录一切，让你之后可以撤销——一条命令、一个文件，或者最近十次操作。

```console
$ rm -rf src/
$ rewind undo
rewind: undone #1 rm -rf src/
$ ls
src/
```

> [!NOTE]
> 没有守护进程，没有 FUSE，也没有 shim 二进制。只有一个静态二进制和一个 shell hook，平时完全无感。

## 为什么需要 rewind

几乎每个人都遇到过：`rm -rf` 删错目录、`mv` 悄悄覆盖配置文件、对未提交的工作执行 `git reset --hard`、一条 `chmod -R` 把自己锁在系统外。备份工具粒度太粗，`trash-cli` 只覆盖 `rm`，而 `git` 只能保护你记得提交的内容。

`rewind` 用一个统一的撤销栈管理它们。

| 能力 | `trash-cli` | `git` | `rewind` |
|---|:---:|:---:|:---:|
| 撤销 `rm` | 是 | 否 | 是 |
| 撤销 `mv` / `cp` 覆盖 | 否 | 否 | 是 |
| 撤销权限 / 所有者变更 | 否 | 否 | 是 |
| 撤销 `git reset --hard` | 否 | 部分 | 是 |
| 撤销 `git clean` | 否 | 否 | 是 |
| 混合命令的撤销栈 | 否 | 否 | 是 |
| 在 Git 仓库外可用 | 是 | 否 | 是 |

## 快速开始

### 安装

最快的安装方式是 Linux 静态二进制：

```bash
curl -fsSL https://github.com/janghotan/rewind/releases/latest/download/rewind-x86_64-unknown-linux-musl.tar.gz \
  | tar -xz -C ~/.local/bin
```

在 ARM64 上把 `x86_64` 换成 `aarch64`。发布二进制静态链接 musl，没有运行时依赖。

<details>
<summary><strong>包管理器与源码构建</strong></summary>

**macOS / Homebrew**

```bash
brew install janghotan/tap/rewind
```

Homebrew 会自动安装 Bash、Zsh 和 Fish 的命令补全。Shell 集成（命令拦截）不会自动启用，需要通过 `rewind init` 添加到 shell 配置文件中（详见下方的[启用 shell hook](#启用-shell-hook)）。

**Fedora / RHEL**

```bash
sudo dnf copr enable janghotan/rewind
sudo dnf install rewind
```

**Debian / Ubuntu**

```bash
sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://janghotan.github.io/apt/rewind-archive-keyring.gpg \
  | sudo tee /etc/apt/keyrings/rewind.gpg >/dev/null
echo "deb [signed-by=/etc/apt/keyrings/rewind.gpg] https://janghotan.github.io/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/rewind.list
sudo apt update && sudo apt install rewind
```

APT 仓库提供 `amd64` 和 `arm64` 静态包，可跨 Debian 和 Ubuntu 版本使用。浏览：<https://janghotan.github.io/apt/>。

发行版软件包会把 hook 安装到 `/usr/share/rewind/`，把补全安装到标准的 shell 目录。

**Cargo** 需要 Rust 1.85 或更新版本：

```bash
cargo install --git https://github.com/janghotan/rewind --locked
```

**从源码构建**

```bash
git clone https://github.com/janghotan/rewind
cd rewind
cargo build --release
install -Dm755 target/release/rewind ~/.local/bin/rewind
```

</details>

### 启用 shell hook

在 shell 的启动文件中加入一行，然后重启 shell 或 source 该文件：

```bash
# bash
echo 'eval "$(rewind init bash)"' >> ~/.bashrc

# zsh
echo 'eval "$(rewind init zsh)"' >> ~/.zshrc

# fish
echo 'rewind init fish | source' >> ~/.config/fish/config.fish
```

hook 会为被包装的命令定义 shell 函数。它们把调用转发给 `rewind exec`，由后者决定是先暂存再运行，还是直接交给真正的二进制。输出、退出码、交互提示和管道行为都与之前完全一致：

```console
$ rm missing.txt
rm: cannot remove 'missing.txt': No such file or directory
$ echo $?
1
```

只有 `reset`、`clean`、`checkout` 和 `restore` 这四个破坏性 `git` 子命令会经过 rewind。其他命令，包括 `git status`、`git log` 和 `git commit`，没有任何额外开销。

卸载时删除启动文件中的那一行，并删除 `~/.local/share/rewind/`。

## 用法

| 命令 | 作用 |
|---|---|
| `rewind undo [N]` | 按从新到旧的顺序撤销最近 `N` 条破坏性命令 |
| `rewind log` | 显示撤销历史；加 `--json` 输出机器可读格式 |
| `rewind status` | 显示数据位置、撤销栈大小、保留策略和已启用的包装器；`--json` 输出机器可读格式 |
| `rewind clean` | 按时间或配置的容量上限清理暂存数据 |

### `rewind undo [N]`

按从新到旧的顺序撤销最近 `N` 条日志记录，默认一条。

```console
$ rm one; rm two; rm three
$ rewind undo 2
rewind: undone #3 rm three
rewind: undone #2 rm two
$ rewind undo --dry-run
would undo #1: rm one (1 item, 2 B)
```

> [!IMPORTANT]
> 撤销支持**断点续做**。如果某条记录的一部分无法恢复——例如你在同一路径重新创建了文件——已经成功恢复的条目会从日志中移除，剩余部分保留，命令随即停止，以免乱序应用更早的记录。解决冲突后再次运行 `rewind undo` 即可。

### `rewind log`

```console
$ rewind log
   ID  WHEN                 STATUS            SIZE  COMMAND
    2  2026-10-21 23:34:11  undone               -  chmod 600 app/config.yml
    1  2026-10-21 23:34:11  undoable          4 KiB  mv new-config.yml app/config.yml
```

`--json` 输出机器可读的记录，包含完整命令、工作目录、状态、条目数量和已暂存字节数。`-n/--limit` 控制显示行数（默认 30）。

| 状态 | 含义 |
|---|---|
| `undoable` | 暂存数据完好，`rewind undo` 可以还原 |
| `undone` | 已经撤销 |
| `running` | 命令仍在执行，或刚刚被中断 |
| `interrupted` | 被包装的命令在完成前被杀死 |
| `failed` | 被包装的命令失败；暂存数据可能仍然保留 |
| `expired` | 暂存数据已被 `rewind clean` 清理 |

### `rewind status`

```console
$ rewind status
rewind 0.1.1

  data      /home/you/.local/share/rewind
  config    /home/you/.config/rewind/config.toml
  journal   /home/you/.local/share/rewind/journal.db (48 KiB)
  trash     /home/you/.local/share/rewind/trash (12 KiB)

  undo      3 commands available
            oldest 2 hours ago
  history   17 total, 13 undone, 1 expired
  staged    12 KiB

  retention 30 days
  wrappers  rm mv cp chmod chown git
  git ops   reset clean checkout restore
  shell     bash hook found
```

### `rewind clean`

按时间清理暂存数据，或强制执行配置的容量上限（优先清理最旧的记录）。日志会保留历史行，并把条目标记为 `expired`。

```console
$ rewind clean --dry-run
rewind: would purge 2 entries (18 MiB)
$ rewind clean --older-than 7d
rewind: purged 2 entries (18 MiB)
$ rewind clean --all
rewind: purged 5 entries (41 MiB)
```

时长单位：`s`、`m`、`h`、`d`、`w`。

### 绕过 rewind

```bash
REWIND_DISABLE=1 rm -rf build/   # 仅对本次命令跳过 rewind
command rm -rf build/            # 绕过 shell 函数
/bin/rm -rf build/               # 绕过所有包装
```

## 覆盖范围

| 命令 | 命令执行前暂存的内容 | 撤销时恢复的内容 |
|---|---|---|
| `rm` | 操作数本身，通过原子重命名移入回收站 | 原始路径、内容、元数据和时间戳 |
| `mv` | 任何将被覆盖的目标 | 覆盖双方；被移动的文件回到源位置 |
| `cp` | 被覆盖的目标文件/目录树，以及新建路径的记录 | 目标原有内容；新建路径会被删除 |
| `chmod` | 每个目标的权限模式，以及相关时间戳 | 之前的模式 |
| `chown` | 每个目标的所有者和所属组 | 之前的所有者和所属组 |
| `git reset --hard` | `HEAD`、索引树和所有未提交更改，固定在 `refs/rewind/*` 下 | 重置前精确的工作区和索引 |
| `git clean -f…` | 每个候选路径的硬链接快照 | 被删除的未跟踪/被忽略文件和目录 |
| `git checkout -- <paths>` | 受影响的工作区文件和索引树 | 之前的工作区内容；索引被恢复 |
| `git restore` | 与 `checkout` 相同，遵循 `--staged` 和 `--source` | 此前的工作区和/或索引状态 |

`rm` 为常用功能实现了与 GNU 兼容的行为，包括 `-r/-R`、`-f`、`-i`、`-I`、`-d`、`-v`、`--`、`--preserve-root` 和 `--no-preserve-root`。rewind 未建模的选项（`--one-file-system`、`--help`、`--version`、未知选项）会直接传给真正的 `rm`，保留原生行为，代价是这一次调用无法撤销。

## 工作原理

1. **暂存。** rewind 记录即将被破坏的内容。对 `rm` 来说，这是一次原子 `rename(2)`，把目标移入回收站——撤销是 O(1) 且字节级精确的，即使目录有 20 GB。对覆盖操作，它会做真正的复制，因为命令可能原地重写文件。对 `git clean`，它使用硬链接。
2. **记录。** 每次调用都会在本地 SQLite 数据库中写入一行，包含命令行、工作目录、状态以及每个条目的暂存元数据。
3. **执行。** 真正的命令继承 stdin/stdout/stderr，其退出码原样返回。
4. **撤销。** `rewind undo` 从新到旧遍历日志并恢复暂存条目，每成功恢复一项就删除对应的日志项。

> [!WARNING]
> 如果无法暂存——父目录只读、空间不足或不支持的选项——rewind 不会阻止命令执行。它会回退到原生行为，并把该条目标记为不可撤销。TTY 上会打印一条警告，告诉你这次调用无法撤销。破坏性命令的能力永远不会比原命令更弱。

### 安全

- 数据目录及其所有祖先目录永远不会被暂存或删除；`rm -rf ~/.local/share/rewind` 会被拒绝。
- 除非真正的工具允许，否则拒绝删除 `/`；`rm` 遵循 `--preserve-root`。
- 回收站和每个条目的目录都以 `0700` 权限创建。
- 如果某个路径在命令执行后已被重新创建，撤销会拒绝覆盖它，并报告冲突，而不是破坏新的工作。
- 没有网络访问，也没有遥测。日志就是一个本地文件。

### 性能

对 `rm` 的暂存是同一文件系统内的一次重命名：几乎零成本，且与目录树大小无关。`cp`/`mv` 覆盖只复制将被覆盖的目标。`git clean` 对候选路径创建硬链接。缓存已热时，每条被包装命令的固定开销是一次 SQLite 事务加进程启动。除四个破坏性子命令外，其他 `git` 命令完全不会经过 rewind。

### Git 快照

`git reset --hard` 是最难处理的情况，因为它可能同时破坏 `HEAD`、索引和未提交的工作。rewind 会用 `git stash create` 创建一个悬空提交，把 `HEAD`、索引和该 stash 固定在 `refs/rewind/<id>-*` 下，防止 `git gc` 回收，然后运行真正的 reset。`git clean` 会对候选集合做快照，并遵循 `-e`、`-X`、`-x`、`-d` 和 pathspec 等过滤条件，无需重新实现它们。`checkout` 和 `restore` 会快照受影响的工作区文件和索引；切换分支、`--patch`、`--merge` 以及交互模式会直接透传。

## 配置

配置是可选的。默认启用所有包装器，并保留回收站数据 30 天。配置文件位于 `~/.config/rewind/config.toml`。

```bash
rewind config init          # 写入默认配置
rewind config show          # 打印生效中的配置
rewind config get wrap.rm   # 读取单个键
rewind config set wrap.rm false
rewind config edit          # $VISUAL / $EDITOR
```

```toml
[wrap]
rm = true
mv = true
cp = true
chmod = true
chown = true
git = true          # git 拦截的总开关
git_reset = true
git_clean = true
git_checkout = true
git_restore = true

[trash]
retention_days = 30 # 设为 0 可禁用自动过期
max_bytes = 0       # 0 = 不限制；优先清理最旧的条目

[journal]
keep_days = 0       # 0 表示永久保留历史记录

[git]
snapshot_untracked = true  # 在 git clean 前快照未跟踪文件
keep_refs = false          # 条目清理后仍保留 refs/rewind/*
```

把某个包装器设为 `false` 是真正的 no-op：`rewind exec` 会在打开日志之前检测到它，并直接执行真正的命令。

| 变量 | 用途 |
|---|---|
| `REWIND_DISABLE=1` | 本次调用绕过 rewind（由 shell hook 处理） |
| `REWIND_DATA_DIR` | 覆盖数据目录（`journal.db`、`trash/`） |
| `REWIND_JOURNAL` | 覆盖日志数据库路径 |
| `REWIND_CONFIG_PATH` | 覆盖配置文件路径 |
| `XDG_DATA_HOME` / `XDG_CONFIG_HOME` | 标准 XDG 覆盖 |

`REWIND_DATA_DIR`、`REWIND_JOURNAL` 和 `REWIND_CONFIG_PATH` 主要用于测试和沙箱环境。

## 限制

这些是刻意的取舍：rewind 宁愿保持正确的原生行为，也不做有缺陷的近似。

- **Linux 是主要目标平台。** macOS 可以构建，文件系统相关命令也能工作，但测试较少；见 CI。
- **非 UTF-8 参数**会直接传给原生命令，该次调用无法撤销。
- **未建模的 `rm` 选项**（`--one-file-system`、未知选项）会直接透传，无法撤销。
- **`rm -r symlink/`**（符号链接末尾带斜杠）会跟随链接；rewind 选择委托给原生工具，而不是猜测。
- **`git clean` 删除的空目录**不会被恢复。
- **子模块状态**以及 `HEAD`/索引/工作区之外的 `.git` 内部状态不会被快照。
- **被中断的命令**（SIGKILL）可能留下 `running` 条目；24 小时后它会被标记为 `interrupted`，已暂存的内容仍然可以恢复或清理。
- **不支持跨用户恢复。** 日志属于执行命令的用户。
- **磁盘占用。** 被删除的文件会一直占用回收站空间，直到运行 `rewind clean`；默认保留 30 天。

## 开发

```bash
cargo build --release
cargo test          # 单元 + 集成测试；封闭环境，无需网络，无需 root
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

集成测试会用真实的二进制在临时 `HOME`/XDG 目录中运行，覆盖 `rm`、`mv`、`cp`、`chmod`、`chown`、git 操作、shell hook、JSON 输出、配置、过期清理以及失败路径。测试框架和发布流程见 [CONTRIBUTING.md](CONTRIBUTING.md)，命令细节见 [man page](docs/rewind.1)，打包脚本见 [`packaging/`](packaging/)。

### 静态 Linux 构建

```bash
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
```

<p align="center"><sub>MIT 许可 · <a href="https://github.com/janghotan/rewind">github.com/janghotan/rewind</a></sub></p>
