# TShell

[English](README.md) | [简体中文](README.zh-CN.md)

TShell 是一个面向 Windows 的 Rust 原生终端工作区。它将本地 Shell、普通
SSH 连接和远程 tmux 会话集中在一个基于
GPUI 的应用中。终端引擎使用 Rust 原生实现和 `alacritty_terminal`，终端
行为以 xterm.js 6.0.0 为兼容参考。

项目仍在持续开发。Windows 是主要验证平台；Linux 和 macOS 的启动路径仍
然保留，但不会持续验证。本 README 描述当前工作树，不承诺完整的 xterm.js
或 OpenSSH 功能兼容性。

## 核心功能

- 本地 PTY 终端和复用连接的普通 SSH 终端。
- 远程 tmux 控制会话，使用服务端原生管理窗口、分屏、布局、标题和工作目录。
- host -> session -> tab -> split 工作区模型，支持可折叠导航、键盘命令、
  列内行自动均分、专注/浮动模式和界面偏好持久化。
- 主机和会话默认按创建先后排列，支持 tmux；主机可在设置中上移、下移，
  会话可拖动标题或通过右键菜单排序，手动顺序按主机保存。
- 紧凑原生控件、简洁文案与按需帮助、紧凑自适应设置、键盘焦点反馈、主机字段校验和
  自适应 Git Diff 控件。加载与错误提示放在内容区，保持工具栏位置稳定。
  刷新保留文件树、Git 列表与滚动位置；文档控件固定占位，会话目录列表随窗口高度扩展，
  补全结果可滚动查看，不再只返回前 8 个目录；状态栏指标按内容宽度紧凑排列。
  命令面板、端口转发、传输队列和 Docker 关联容器列表充分利用可用高度，Docker JSON 表单随窗口调整。
  详见 [工作区界面](docs/workspace-interface.md)。
- 面向本地和远程工作目录的 Explorer 与只读 Git 视图。远程文件使用 SFTP，
  远程 Git 使用目标主机上的 Git，不需要远程 Python 或文件挂载。
  并排和行内差异支持选择、复制文本及字符级变更高亮。
  详见 [Git 工作区](docs/git-workspace.md)。
- Docker 工作区，支持本地及已保存 SSH 主机的 Engine：可展开的容器 / Compose /
  镜像树、完整容器列表、独立镜像标签行与自适应详情，以及资源操作、日志、内嵌
  临时容器终端、Compose YAML 和带冲突检测及备份恢复的 daemon JSON 编辑。随当前
  主机切换，不依赖终端会话。
  详见 [Docker 工作区](docs/docker-workspace.md)。
- 内置 UTF-8 文本编辑器、后台语法高亮、搜索和带冲突检测的远程保存。
- 全局 SFTP 传输队列，支持并发上传/下载、目录扫描、字节进度、拖动面板、
  自动收起到右侧、速度与剩余时间，以及单个任务取消、清除和重试。
  文件和文件夹下载默认保存到系统下载目录。
  详见 [SFTP 传输](docs/sftp-transfers.md)。
- Markdown 使用原生富文本预览；HTML 和 PDF 在 Windows 上使用只读 WebView2 预览。
  HTML 可一键切回编辑，PDF 为只读。HTML 预览不会执行脚本或加载外部资源。
- 工作区与终端共用色调，目前内置 VS Code Modern 的亮色/深色方案。
  同一个 `theme.json` 为每套方案保存终端色与界面语义色，旧版文件自动迁移。
  自定义配色方案单独编辑，主题可分别指定明色和暗色方案；白天、夜间或
  自动模式决定实际使用哪套。自动模式只读取 Windows 的
  明暗设置。表单弹窗使用单层标题和统一间距，表面跟随当前配色；小表单操作使用带中文提示的图标，确认提示保持紧凑。字体、行高、编程连字、透明度、
  亚克力背景和界面语言均可配置。外观设置按主题、字体、窗口、背景分组，
  不为每个控件单独建组。
- 参考 xterm 的键盘、粘贴、选区、ANSI/256 色/真彩色 SGR、自定义框线字形、
  光标样式、OSC 7 工作目录元数据、OSC 52 剪贴板写入和 OSC 9/777 原生通知。
- 面向 TUI 的常用调色板、前景/背景和光标颜色查询、设置与重置，以及主题变化通知。
  支持范围及限制见 [颜色控制](docs/terminal-colors.md)。
- 当前分屏的屏幕与保留历史输出搜索，提供匹配数、大小写/正则选项和上下一处。
  Ctrl 点击可打开 OSC 8 链接、URL 与文件路径，SSH 路径按输出所属主机打开。
  Windows 通知点击可在应用运行时定位原分屏。详见 [终端工具](docs/terminal-tools.md)。
- 通过现有 SSH 连接采集 Linux 主机 CPU、内存和根文件系统使用率，不需要 sudo
  或 Python。
- 紧凑的全局列表统一管理本地、远程和 SOCKS5 SSH 转发。添加和编辑共用行内类型、SSH 主机
  和端口控件，通过图标确认或取消；保存的规则随 TShell 启动。通过底部状态栏图标打开转发列表。详见
  [SSH 端口转发](docs/ssh-forwarding.md)。
- Windows x64 打包和基于发布元数据校验的后台更新路径。详见
  [Windows 发布说明](docs/windows-release.md)。

## 环境要求

Windows 开发需要安装：

- 带 MSVC 工具链的 Rust
- Visual Studio C++ Build Tools
- 用于读取 SSH 配置的系统 OpenSSH 客户端

仓库包含 release 构建使用的固定版本 Windows GPUI 补丁和 ConPTY 运行库。远程
tmux 主机需要安装 tmux；远程 Explorer 和 Git 分别需要目标主机提供 SFTP 和 Git。


Docker 需要运行中的本地 Engine 及管道/socket 访问权限；SSH Docker 需要远端主机安装
Docker CLI 并有权访问其 Engine。本地 SDK 操作无需 Docker CLI；Compose、镜像 push
需要目标主机的 CLI，Compose 还需要相应插件，Desktop 重启也使用 CLI。配置编辑和服务
控制需要当前账号的权限。可指定本地管道/socket 路径。

## 构建和运行

```powershell
cargo run --locked --bin tshell
```

调试程序位于 `target/debug/tshell.exe`，默认启动本地 PowerShell。创建 release
构建：

```powershell
cargo build --locked --release --bin tshell
```

TShell 将主机和界面偏好保存到系统配置目录下的
`tshell/workspace.json`。设置 `TSHELL_DATA_DIR` 可以为开发或 UI 检查使用独立
目录。密码不会写入该文件。

## 工作区模型

侧栏从主机开始。每个主机包含多个 session，每个 session 包含多个 tab，每个
tab 包含一个或多个 split。本地和普通 SSH session 会保存名称和起始目录，并在
下次启动时创建新的进程。tmux 在远程服务器上保存 session、window、pane、布局
和工作目录；TShell 不会在本地配置中复制这些快照。

主机切换框保持原来的样式。下拉列表为每台主机显示连接状态点，并在每个已连接
的远程主机项后面显示断开按钮。按钮只断开对应主机，不切换当前主机，也不删除
主机配置；再次选择该主机即可重新连接。断开普通 SSH 会关闭其终端，
tmux 会话则保留在服务器上。断开后，文件浏览器和 Git 不会访问该主机；重连后
可恢复原文件草稿。会话右键菜单提供“删除会话”；如果会话中有未保存的文件编辑
或文件操作正在进行，删除会话及关闭最后一个标签页或分屏都会被阻止。
修改主机连接或移除主机前，请先保存已打开的文件。如果对应主机仍有未保存编辑，
或文件操作尚未完成，TShell 会阻止这些动作。断开同一主机会保留文件草稿。
连接参数改变后会清空该主机的文件和工具视图，避免继续使用旧连接的路径。

使用 `Ctrl+Shift+K` 打开命令面板，主机、会话、操作分组显示在同一个列表里。
添加主机、终端与分屏、文件与 Git、设置等功能统一放在操作里。
列表用勾选标记选中项，会话只显示所选主机下的会话，按侧栏顺序排列。
支持按名称、地址、目录或命令搜索，用方向键选择、Enter 执行。常用默认快捷键如下：

| 快捷键 | 操作 |
| --- | --- |
| `Ctrl+B` | 折叠或展开侧栏 |
| `Ctrl+1` / `Ctrl+2` / `Ctrl+3` | 终端 / Explorer / Git |
| `Ctrl+Shift+T` / `Ctrl+Shift+W` | 新建 / 关闭终端 tab |
| `Alt+Enter` | 专注当前 split 或恢复布局 |
| `Alt+F` | 浮动当前 tab 或恢复平铺 |
| `Alt+1` … `Alt+9` | 按侧栏顺序切换主机下的第 1–9 个会话 |
| `Alt+N` | 新建列并等分所有列 |
| `Alt+Shift+N` | 在当前列新建行并等分该列 |
| `Alt+H/J/K/L` 或 `Alt` + 方向键 | 移动焦点 |
| `Alt+Shift+H/J/K/L` 或 `Alt+Shift` + 方向键 | 移动 split |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | 复制 / 粘贴 |
| `Ctrl+Shift+F` | 搜索当前终端输出 |
| `Ctrl+Shift+P` | 切换 GPUI 帧性能面板 |
| `Ctrl+-` / `Ctrl+=` | 减小 / 增大终端字号 |

分屏采用等宽列、列内等高行。关闭分屏或 shell 自行退出后，剩余组会自动重新均分。
不再提供拖拽分隔线、分屏大小快捷键或其他平铺布局。
本地、普通 SSH 和 tmux 使用同一套模型，详见
[均分布局](docs/workspace-interface.md#equal-splits)。
TShell 在客户端计算并向 tmux 提交明确的窗格宽高，混合新建、关闭行列后仍保持均分。
重连时空白终端会显示当前 tmux 阶段；布局命令与外部变化冲突时会刷新窗格 ID 并重试，
窗口过小时会提示无法容纳均分分屏。

所有操作都列在设置中并可以重新绑定。工作区快捷键是应用契约，不会因为 xterm
兼容性工作而被替换。
终端状态栏项目（包括可选的 SSH/tmux RTT）在设置中配置；RTT 测量的含义见
[性能诊断说明](docs/performance-diagnostics.md)。

## SSH 和 tmux

SSH 主机可以填写地址、用户、端口和私钥，也可以通过 `ssh -G` 解析现有的系统
OpenSSH 配置。终端 pane、tmux 控制、SFTP、指标采集和短命令使用相互隔离的
SSH 连接池，避免某个工具耗尽其他工具的 channel 配额。

每个 tmux session 使用一条专用控制连接。pane 输出、标题、布局和外部变更通过
tmux 控制协议同步。关闭 TShell 只会断开客户端，远程 tmux session 会继续运行。
未启用 tmux 的主机为每个 pane 使用独立的 SSH PTY channel。

主机密钥验证取决于连接路径。直连主机会将已接受的指纹保存到本地
`known_hosts`；使用 OpenSSH 配置的主机沿用系统配置及其 agent/proxy 设置。TShell
不承诺支持所有 OpenSSH 选项、用户证书或主机 CA 行为。

## Explorer 和 Git

Explorer 扫描有上限的文件树，并打开不超过 20 MiB 的普通 UTF-8 文件。文件树会
跳过 `.git`、`artifacts`、`node_modules` 和 `target`。远程读写使用 SFTP；远程
保存会先比较原始内容，在目标旁边写入临时文件，并在服务器支持
`posix-rename@openssh.com` 时原子替换目标文件。发现外部修改时会提示冲突，不会
静默覆盖。

Markdown（`.md`、`.mdx`）和 HTML（`.html`、`.htm`）文件在不超过 256 KiB 时默认显示只读预览；PDF（`.pdf`）文件使用 WebView2 预览，大小上限为 20 MiB；超出上限的文件仍在源码编辑器中打开。
Markdown 原文在所有平台上直接由 GPUI Kit 的原生 `TextView::markdown` 渲染。
Windows 上的 HTML 使用内嵌 WebView2，可显示布局和内联 CSS；PDF 使用 WebView2 内置的 PDF 查看器。HTML 预览禁止执行脚本、加载外部资源、提交表单和跳转；相对路径资源也不会加载，包括远程文件的资源。
其他平台或没有 WebView2 的系统会显示 PDF 运行时提示，并对 HTML 使用文本导向的原生后备预览。文件头部的“编辑/预览”
按钮可切换视图，保存时始终写回原始 UTF-8 源码。

Git 视图显示当前工具目录的 `git status --porcelain`。它是只读浏览器，不提供暂存、
提交、推送或历史编辑；这些操作请使用命令行。
两种差异布局均可拖选文本，并通过 `Ctrl+C`、自定义复制快捷键或工具栏复制按钮复制。
复制结果不包含行号与补丁标记，保留制表符和 Unicode。

## 终端兼容性

兼容性基线和验证证据维护在
[docs/xterm-compatibility.md](docs/xterm-compatibility.md)。该文档记录固定的
xterm.js 源码链接、已提交的 fixture、渲染行为和有意扩展。当前重点包括：

- ANSI 16 色、256 色、真彩色、粗体增亮、反色、半透明淡色和多种下划线样式。
- 基于固定参考用例的键盘、粘贴、括号粘贴、鼠标、焦点、光标、选区、IME 和
  Unicode 处理。
- 用于框线、块、阴影、弧线和对角线的格子级自定义字形。
- OSC 标题和工作目录元数据、OSC 52 剪贴板写入以及桌面 OSC 9/777 通知。
- TShell 和支持 tmux 的应用使用的主题查询与订阅扩展（`CSI ?996n`、
  `?2031h` 及相关响应）。

终端状态引擎仍是 Alacritty parser/grid。浏览器原生字体栅格化、所有 VT 序列、
所有 OpenSSH 键盘布局和复杂终端恢复路径尚未全部进行差分验证。修改终端行为前，
请先阅读兼容性文档。

## 架构

- `src/backend.rs` 和 `src/backend/`：类型化工作区操作、快照、生命周期和分屏几何。
- `src/terminal.rs`、`src/terminal/` 和 `src/terminal_view/`：PTY 与终端状态、
  渲染、选区、字形和协议观察器。
- `crates/tshell-host/` 管理 SSH 配置、连接池、认证、转发、SFTP 和文件操作，不依赖 UI。
- `crates/tshell-ui/` 管理共享控件、语义调色板和本地化；根 `locales/` 是唯一翻译来源。
- `src/host.rs` 保存主机名称和 tmux 偏好，与 SSH 连接配置分开。
- `src/tmux_client/`：tmux 控制流和同步。
- `src/workspace/`：主机、session、设置、Explorer、Git、指标、编辑器状态
  和更新界面。
- `src/terminal_theme/`：内置和自定义终端配色。

仓库布局、UI 边界、Rust 风格、命名规则和贡献约束见
[AGENTS.md](AGENTS.md)。

## 测试和验证

运行常规测试：

```powershell
cargo test --workspace --locked
```

重点检查包括：

```powershell
cargo test --locked xterm_compat
cargo test --locked terminal_notifications
cargo test --locked osc52
```

可选的真实主机覆盖需要一个已有的免密 SSH 别名：

```powershell
$env:TSHELL_SSH_TEST_HOST = 'your-ssh-alias'
cargo test --workspace --locked -- --include-ignored
```

可以在 Node 24+ 和网络可用时通过 `tools/generate-xterm-fixtures.mjs` 重新生成
兼容性 fixture；普通 Rust 测试只使用仓库中的 fixture。开发构建可通过
`tshell.exe --workspace-ui-check <report.json>` 运行隐藏窗口 UI 检查。

## 打包和更新

TShell 0.1.4 的变更见 [发布说明](docs/release-notes.md)，包括 tmux 布局恢复、
主机和会话排序持久化、自适应列表，以及 Windows HTML 预览崩溃修复。

推送 `v*` tag 会运行 Windows GitHub Actions，执行测试、构建、打包并发布同仓库
Release。产物包含程序、许可声明、校验和与更新元数据。工作流和恢复方式见
[docs/windows-release.md](docs/windows-release.md)。

Windows release 构建通过 HTTPS 检查公开的 Release 元数据，验证版本、架构、文件
大小和 SHA-256，然后把更新暂存到下一次启动。更新流程不会为二进制签名或强制重启，
也不会保存 GitHub 凭据。

## 许可证

TShell 使用 Apache License 2.0。第三方声明和 vendored 组件许可证包含在仓库中。
