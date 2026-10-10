# TShell 架构重构与 Docker 实施计划

更新日期：2026-10-10。状态：模块拆分、Docker 主机联动、四类资源界面与管理功能已完成。workspace 自动化回归和 Windows 原生界面检查已通过，真实 Docker/SSH 环境验收单独保留。

当前新增需求以 [Docker 资源工作区设计](docs/docker-resource-design.md) 和下文第 9 节为准：容器 / Compose / 镜像 / 更多四类入口，去掉重复标题、侧栏状态筛选和数量文案；资源预览集成操作与 Engine 配置编辑。

本计划替换上一版。目标是直接建立清晰的最终结构，允许重写当前内部架构；继续保留用户配置、未保存文件、运行中的终端和已有功能行为。所有工具仍放在左侧，Docker 独立于终端会话。

## 1. 重构原则

1. 按真实职责拆分：主应用负责组合，连接库负责 IO，UI 库负责共享界面，工具 crate 负责自己的业务与视图。
2. 共享具体实现，优先 enum、struct、方法和 GPUI 类型化事件。只有存在真实替换需求时才引入 trait。
3. 直接使用成熟库：Docker 使用 Bollard 的连接、模型、请求和流，不再包一套通用 Docker SDK。
4. 最终代码只有一套所有权和执行路径。允许分阶段验证，但不围绕旧结构建设长期兼容层。
5. 稳定身份、异步取消、正确路径和状态恢复是实际行为要求；不为这些要求建立通用插件框架。
6. 模块化服务于 TShell 的本地与远端工作流，不要求独立产品、动态插件、跨 UI 框架或任意 backend 替换。

## 2. 最终仓库结构

```text
Cargo.toml                    # 根应用与 Cargo workspace
Cargo.lock                    # 唯一锁文件
src/
  main.rs                     # 启动与依赖组装
  workspace.rs                # 左侧导航、页面分发、宿主事件
  workspace/                  # 会话、设置、全局传输队列等应用功能
  backend.rs / backend/       # 会话与分屏动作
  terminal.rs / terminal/     # 终端状态、输入输出与连接桥接
  terminal_view/              # 终端渲染
  tmux*.rs / tmux_client/     # tmux 协议与服务端状态同步
crates/
  tshell-host/                # 主机身份、SSH、SFTP、本地/远端文件与命令
  tshell-ui/                  # 公共主题 token、控件、字体配置、本地化
  tshell-explorer/            # 文件导航、编辑器、预览、文件操作
  tshell-git/                 # Git 查询、解析、列表、Diff 与文本选择
  tshell-docker/              # Bollard 客户端、目标、资源视图与容器终端
locales/                      # 翻译唯一来源
docs/                         # 已实现行为和限制
```

| Crate | 公开的实际能力 | 明确的边界 |
| --- | --- | --- |
| `tshell-host` | 稳定 `HostId`、SSH 配置、可共享连接 handle、命令执行、文件访问与路径类型 | 不含 GPUI、会话布局、tmux、Docker 协议或应用配置 UI |
| `tshell-ui` | 界面语义颜色、通用控件/尺寸、本地化入口 | 不含连接、终端协议、业务目标或工具注册机制 |
| `tshell-explorer` | Explorer Entity、文件浏览/编辑状态、宿主请求事件 | 直接调用 host；文件选择器与全局传输调度交给主应用 |
| `tshell-git` | Git Entity、命令/解析、Diff 模型与宿主请求事件 | 直接调用 host；打开文件交给主应用 |
| `tshell-docker` | Docker Entity、Bollard 连接、资源操作、可交互容器终端 | 本地直接使用 Bollard；SSH 只增加必要 connector；打开终端交给主应用 |

依赖方向：

```text
tshell 应用
  ├─ tshell-host
  ├─ tshell-ui
  ├─ tshell-explorer ── host + ui
  ├─ tshell-git      ── host + ui
  └─ tshell-docker   ── host + ui + bollard
```

三个工具相互不依赖。共享库包含真实实现，不是注入服务的空接口集合。使用两个共享 crate 是为了让连接代码保持不依赖 GPUI，并统一三个工具的实际界面基础。

根级管理 `[workspace.dependencies]`、vendor patch 与发行配置；内部 crate 设置 `publish = false`。保持根应用入口与浅目录，文件随真实职责拆分，不预先创建空目录或每功能一组 core/ui crate。不强制为每个工具增加 `ui` feature；纯逻辑测试在所属模块中完成，诊断/截图 feature 按现有实际需求透传。

## 3. 主应用与工具的组合方式

### 3.1 一个页面选择，三个具体 Entity

主应用使用一个 `Page::{Terminal, Explorer, Git, Docker}` 枚举选择左侧与主区，直接 exhaustive match。所有工具入口放在左侧切换栏。

- Terminal：左侧主机/会话导航，主区终端和分屏。
- Explorer：左侧目录树，主区编辑器/预览。
- Git：左侧变更列表，主区 Diff。
- Docker：共用主机选择器，左侧容器 / Compose / 镜像 / 更多资源列表，主区资源预览、编辑器与操作。

每个工具提供少量具体方法：绘制侧栏、绘制主区、提供状态栏内容、激活/失活、刷新和焦点处理。主应用持有对应 Entity 与 Subscription，用 GPUI 类型化事件协调平台操作和跨工具导航。

当前需求不需要独立的 `SidebarMode + MainRoute`、通用 `Tool` trait、插件注册器或全局事件总线。页面选择不包含业务状态，也不决定 Docker 连接目标。

### 3.2 会话上下文与 Engine 分开拥有

| 状态 | 所有者与恢复范围 |
| --- | --- |
| 主机配置、会话、标签页、分屏、窗口和快捷键 | 主应用与现有 backend/tmux 边界 |
| 会话对应的工作目录与 Explorer/Git Entity | 主应用的 `SessionWorkspace` |
| 文件树、文档、未保存内容、编辑器与预览 | 所属 Explorer Entity |
| Git 变更、Diff、选择、滚动与布局偏好 | 所属 Git Entity；需要跨会话共享的现有偏好由应用保存 |
| 当前 Engine 目标 | 主应用从当前 HostId 派生，复用左侧主机选择器 |
| Engine 连接、容器选择、日志与操作 | Docker Entity，按 HostId 保存浏览状态 |
| 上传/下载排队与全局任务 UI | 主应用的全局传输队列 |

`SessionWorkspace` 将一个会话的工作目录和两个工具实体放在一起，直接替代主应用当前分散的文件/Git 状态与恢复映射。工具只接收 host handle 与目录，不接收 `AppView`、backend、tmux 或完整会话对象。实体按实际使用惰性创建；页隐藏时隐藏原生预览窗口、停用视图专属任务，关闭会话前处理未保存内容。

Explorer/Git 首次打开时由主应用从终端解析目录，随后保持该会话的工具根目录；不因 pane 切换或 shell `cd` 自动丢失浏览/编辑状态。目录变更经宿主协调，两工具获取相同工作目录，Git 自行定位仓库根。

Docker 跟随工作区共用的主机选择器：本地主机对应本地 Engine，SSH 主机对应远端 Engine；每个主机保留自己的容器选择、筛选、详情页和日志状态。同一主机内切换或关闭终端会话不会切 Engine，没有终端会话也能连接 Docker。Docker 页切换主机不创建 shell/tmux 会话，面板一直显示真实 Engine 来源。

### 3.3 事件只用于宿主边界

- Git 的“打开文件”：模块事件携带文件定位；宿主订阅时固定来源 `SessionWorkspace`，交给该 Explorer，先处理未保存确认。
- Explorer 的“打开目录终端、上传/下载、系统定位”：交给主应用执行平台动作或提交全局任务。
- Docker 的“打开容器终端”：携带原 Engine 与容器完整 ID，由宿主创建终端标签。
- 刷新、查询、Diff 投影、文件读写、容器启停等模块内部行为直接调用具体服务。
- 异步请求和确认捕获原始目标；上下文已变更或目标已删除时报告失效，不重新读取当前选中对象来执行。

## 4. 共享 host 库：直接拆出实际实现

当前 `HostConfig` 在 `tmux_client.rs`，SSH/SFTP 又反向依赖它。彻底拆开：

- 主应用保存 `HostDefinition`：稳定身份、显示名、连接配置、是否使用 tmux 等应用选项。
- host 库保存独立 `SshConfig` 与连接 handle；tmux 选项不进入通用 SSH 配置。
- `src/ssh_pool/` 的连接、认证、通道和 runtime 迁入 host；UI 使用认证请求通道回应密码、交互认证与主机密钥确认。
- `remote_files/` 的 SFTP IO 迁入 host；文件模型和文件浏览/编辑逻辑进入 Explorer。
- 本地与远端通过具体 `Host/Connection` 枚举分发，提供 `run`、文件访问和 SSH 通道等实际方法。不再构造 `CommandExecutor`、`FileService`、`ByteStreamFactory`、`Localizer` 的 trait 注入体系。
- 命令参数、stdin、允许退出码、上限与超时使用小型请求结构；本地直接 argv，远端集中处理 POSIX quoting。Git 的批量命令保留清晰的执行顺序与退出码语义。
- 本地路径和远端 POSIX 路径用明确类型/enum 区分；连接与文件定位保持一致，不在 Windows 对远端路径调用本地路径运算。
- 主机稳定身份直接用于配置、会话所属关系与工具目标；迁移旧配置，内部不长期并存“主机索引身份”和“新身份”两套 API。
- runtime 与通道寿命由连接库管理，模块按具体任务持有 handle。连接断开、配置变更和删除有明确失效行为；网络 IO 不阻塞 GPUI 线程。

不借本次拆分重写已验证的 SSH 协议、认证算法或 tmux 所有权；重构配置、类型与调用关系即可。终端状态、渲染和布局也继续保持独立模块，只有新增容器终端接入所需的边界需要调整。

## 5. Docker：Bollard 能做的直接用

### 5.1 能力分工

已核实的发布版本为 Bollard 0.21.1；实施时锁定具体版本与 Cargo.lock，不根据 upstream main 假设接口。

| 需求 | 实现方式 |
| --- | --- |
| Windows Named Pipe、Unix Socket | Bollard 现成连接方法 |
| HTTP/TLS endpoint | Bollard 现成方法；产品入口可随后开放 |
| Engine API 版本协商 | `negotiate_version` |
| 容器/镜像/卷/网络查询与操作 | Bollard API 和模型 |
| 日志、events、stats | Bollard 提供的流 |
| 容器交互终端与 resize | Bollard create/start/resize exec |
| TShell 已保存 SSH 主机 | 一个小型 connector，将 host 的 SSH 通道接入 Bollard custom transport |
| 从 Compose YAML 执行 up/build/down | 后续使用执行端 Compose CLI |

Docker crate 直接持有 `bollard::Docker`，内部直接调用。列表/详情只生成必要显示数据，不复制整套 Bollard DTO，也不再包一层与 Bollard 同名同签名的服务 API。流消费、加载状态和 UI 更新是模块自己的工作。

### 5.2 本地连接保持简单

- 初版支持本地默认连接与用户明确填写的 pipe/socket endpoint。
- 用 Bollard 原生方法连接，协商版本并查询服务端 OS；Engine 不可用时提供明确错误与重连。
- “本地”目标使用本地连接方法，不因 `DOCKER_HOST` 指向远端而偷偷变成另一台机器。
- 验证 Docker Desktop 当前 Engine 和默认 pipe 的实际行为，默认不可用时允许明确 endpoint。
- 不把完整 Docker context 导入、环境变量优先级、证书配置编辑器设为第一版前置任务；后续用户需要选择 context 时再补一个小型解析模块。
- Bollard 0.21.1 的默认连接读取 `DOCKER_HOST`/平台默认值，不能宣称它已经完整处理 Docker CLI context。

### 5.3 SSH 只补必需的 connector

Bollard 的内置 SSH 使用 `openssh`。该依赖使用 Unix 接口，且只支持无需交互输入的认证；不适合 Windows 主平台和 TShell 的密码/交互认证。因此，TShell 的 SSH 目标使用已有 `russh` 能力，补充 connector 是为了补实际缺口。

```text
Bollard 请求/日志/exec
    → 小型 HTTP connector
    → tshell-host SSH 通道
    → 远端 docker system dial-stdio
    → Docker Engine
```

connector 放在 Docker crate 的一个聚焦模块中：复用 host 的认证与异步通道，以 Hyper 完成 HTTP 与 upgrade，再接入 Bollard custom transport。Bollard 不负责 TShell 主机选择，host 库不理解 Docker HTTP。

远端需要 Docker CLI 和 Engine 访问权限；本地不需要 Docker CLI，不需要开放远端 Docker TCP 端口或先创建 shell。通道不分配 PTY，HTTP stdout 和 stderr 分离。使用普通通道上限与明确取消；只有实际证明竞争严重时再细化调度，不先建立 Docker 专用连接管理框架。

实施时先验证 ping/list、持续日志、HTTP upgrade、exec 双向输入输出和 resize。列表能返回不代表交互终端已经验证；无需为这些验证单独建设长期原型项目。

### 5.4 第一版范围

- 所有容器列表、搜索/状态过滤、Compose labels 分组。
- 概览：状态/健康检查、镜像、端口、挂载、创建/启动时间。
- 启动、停止、重启、删除已停止容器；操作确认绑定原目标，完成后读取实际状态。
- 日志 tail/follow、暂停/恢复、搜索、复制、自动滚动；有界缓存、取消和批量 UI 更新。
- events 合并触发刷新，断线恢复重新读取完整快照；查询与操作各自显示状态。
- 容器终端固定持有创建时的 Engine/exec；切换 Docker 面板目标不关闭已有终端，关闭终端不停止容器。
- 命令兼顾 Linux/Windows 容器；直接连接 Engine exec，不向用户现有 shell 注入命令、不创建伪 tmux pane。
- 网络中断后结果不明的变更操作先核实状态，不自动重放。

镜像管理、stats、卷/网络管理、完整 context/TLS 设置和 Compose 编排放在后续。Compose 的执行机器、配置文件路径和目标 Engine 显式绑定，bind mount 仍按 Engine 主机解释。

## 6. 执行 TODO

允许重写模块、重新命名类型、调整配置与状态结构。按最终依赖方向直接迁移，在同一工作分支逐阶段验证；完成后删除旧实现，不保留两套长期运行路径。以下顺序表示集成依赖，不要求按旧版逐层服务适配。

### 阶段一：建立 workspace 与真实共享能力

- [x] 记录当前工作树和已有功能测试基线，保留全部未提交工作与用户运行实例。
- [x] 建立 root workspace、共享依赖与内部 crate；保留入口、vendor patch、锁文件和发行构建。
- [x] 建立 tshell-host：拆开应用主机定义与 SSH 配置，迁移稳定身份、runtime、SSH 连接/认证、通道与 SFTP。
- [x] 为现有终端、tmux、转发和指标调用更新 host 类型/路径，验证原有连接行为。
- [x] 建立 tshell-ui：共享控件、语义 token 与本地化；根 locales 保持唯一来源，语言/主题即时生效。
- [x] 完成旧配置迁移和新 API；移除重复 HostConfig/身份/翻译初始化。

当前落地：

- 基线提交 `775ca51`，共享层提交 `d7ea914`；后续实现沿同一 `codex/workspace-modules` 分支完成。
- workspace 已包含根应用与五个内部 crate，共用 Cargo.lock、GPUI 依赖和 vendor patch。
- `HostDefinition` 持有稳定 `HostId` 与显示/tmux 偏好，`SshConfig` 仅包含连接字段。旧扁平 JSON 和按 endpoint 保存的会话/转发自动迁移；重复旧 endpoint 获取独立 ID 并保留原会话资料。
- host 拥有本地/SSH 命令、有界输出和超时、Local/POSIX 路径、runtime、连接/认证/转发/SFTP，以及不含 PTY、隔离 stderr、支持取消的 stdio stream。
- UI 拥有真实共享控件、语义 palette、编辑器高亮与唯一翻译初始化。host 诊断保留类型和源错误，在 UI 显示时翻译。
- 原终端、tmux、转发、指标与传输边界保持原所有权；工具代码已移至各自 crate，根目录旧实现已删除。

验收：host 不依赖 GPUI/tmux；主应用与既有终端/SSH 工作流可用，配置兼容，真实连接实现只有一份。

### 阶段二：重写工作区组合并提取 Explorer/Git

- [x] 用 Page、SessionWorkspace 和具体 Entity 替代当前分散状态与恢复映射。
- [x] Git 迁入独立 crate：命令、status/numstat/patch、Diff 投影、高亮、文本选择和虚拟列表。
- [x] Explorer 迁入独立 crate：树、文件操作、文档、搜索、冲突保存与预览；接入具体 host API。
- [x] 移除 git 与 workbench 的交叉依赖和工具对 AppView 的直接访问。
- [x] 用模块事件接通 Git 打开文件、目录终端、文件选择和全局传输队列。
- [x] 左侧统一接入三个已有页面；实现激活/失活、焦点、快捷键、未保存关闭和预览子窗口生命周期。
- [x] 清理旧 impl AppView 工具方法、工具状态字段和临时桥接；保留既有用户功能。

验收：两个工具拥有各自状态，宿主只组合；Git/Explorer 测试、原生选择/复制/滚动、未保存恢复与 SFTP 流程通过。

### 阶段三：直接完成 Docker 连接与基础页面

- [x] 锁定 Bollard 发布版本；直接接通本地默认/显式 endpoint、版本协商与容器列表。
- [x] 实现小型 russh connector；协议 fixture 验证版本协商、HTTP upgrade、exec 输入/resize/关闭、响应前取消，以及真实 loopback russh stderr 隔离与通道关闭。
- [ ] 真实 Engine 验收：Windows 本地及 SSH 目标上的日志/exec/resize、权限与断线。当前机器没有 Docker，未配置测试 SSH 主机。
- [x] 接入按 HostId 定位的 DockerTarget、共用主机选择器和左侧 Docker 页面；不要求存在终端会话。
- [x] 完成容器列表、分组、概览和按 Engine 保存的选择/过滤状态。
- [x] 完成主机配置变更/删除、Engine 切换、旧请求失效和显式重连。

验收：本地与 SSH 使用同一套 Bollard 业务调用；Docker 随主机切换，按主机保留浏览状态，同一主机内不随终端会话变化。

### 阶段四：完成 Docker 日常操作与容器终端

- [x] 完成启停/重启/删除、原目标确认、busy/错误反馈与实际状态刷新。
- [x] 完成日志阅读、follow、暂停/恢复、搜索、复制、有界缓存和事件合并刷新。
- [x] 接入 ContainerTerminal 的 IO 与 resize 到现有终端契约，创建具有明确来源的标签页。
- [x] 实现面板目标隔离、终端取消、已停止容器 exec 拒绝与原目标结果反馈，覆盖自动化 fixture。
- [ ] 在真实隔离容器上验证外部删除、缺失 shell、权限不足、Windows 容器及断线后的结果。
- [x] 完成深浅主题、左栏折叠、窄窗口和大量资源/日志的原生 UI 检查。

验收：本地与 SSH 都完成“查看 -> 日志 -> 启停 -> 容器终端”；任务可取消、目标不串线、隐藏工具不破坏既有终端。

### 阶段五：清理、回归与文档

- [x] 删除冗余模块和旧恢复映射，检查无反向依赖、重复实现或无调用方的公共抽象。
- [x] CI 从仅测根 bin 改为覆盖 workspace，透传原生 UI 截图 feature，检查 Windows 发行构建与独立目录打包。
- [x] 更新 AGENTS.md 的目录与所有权、workspace/Git/SFTP 文档，新增 Docker 实际使用与限制文档。
- [x] 同步 README.md、README.zh-CN.md 与 locales，记录真实验证平台、Engine/CLI 版本和未验证边界。

验收：最终结构简洁且只有一套执行路径；独立 crate 可测试，当前功能、配置与用户内容都得到保留。

实施取舍：

- 容器 exec 挂到现有 local backend 中具有明确来源的临时会话；同一 Engine endpoint/容器复用会话增加标签。不同来源不合并，不恢复为普通 Shell，也不向现有 Shell 注入命令。容器会话不开放普通 Shell 标签/分屏或主机 Explorer/Git；新增容器终端从 Docker 页面打开。
- 日志使用 Bollard tail/follow，暂停取消、恢复重新读取 tail 1000；保留最多 2 MiB 与 10,000 个换行。输入和 resize 复用终端契约，最新 resize 不因输入队列拥塞丢失。
- 没有引入 Tool trait、动态插件注册器或 Docker DTO/SDK 包装；Bollard 提供的 API/模型直接使用。
- 详细行为、要求与真实环境验证缺口见 [Docker workspace](docs/docker-workspace.md)。

## 7. 验证与完成标准

实施中执行 `cargo fmt --all`；workspace 建立后完成：

```powershell
cargo fmt --all -- --check
cargo test --workspace --locked
cargo check --workspace --locked --all-targets
cargo build --locked --release --bin tshell
```

原生 `--workspace-ui-check` 使用隔离 TSHELL_DATA_DIR；截图和报告放在 artifacts/。不覆盖锁定的运行程序；需要时使用独立构建目录。

本次验证记录（Windows x64，2026-10-09）：

- `cargo fmt --all -- --check` 与 `cargo check --workspace --locked --all-targets --features ui-check-screenshots` 通过。
- `cargo test --workspace --locked`：221 通过，20 忽略；忽略项需要显式远端环境。
- 原生 `--workspace-ui-check` 通过；深/浅主题、860 x 520 窄窗口、万容器列表、日志/配置 editor 的真实主区高度、侧栏折叠、主机切换与无终端会话均已检查。使用隔离 `TSHELL_DATA_DIR`；跳过系统剪贴板以保留用户内容。
- `cargo build --locked --release --bin tshell --target-dir target/module-check` 通过；未覆盖用户运行中的 debug/release 可执行文件。
- 使用 `tools/package-windows.ps1 -Executable target/module-check/release/tshell.exe -OutputDirectory artifacts/module-release-package` 完成独立 Windows 发行打包；EXE、ZIP 内 EXE、更新 JSON 和 SHA256SUMS 哈希一致。
- 详细本地报告：`tmp/modularization/final-tests.log`、`tmp/modularization/final-check.log`、`tmp/modularization/release-verified.log`、`artifacts/module-complete-ui.json`；画面在 `artifacts/module-complete-screenshots/`。这些生成产物不提交。
- 当前机器没有 Docker Desktop/Engine，也未配置 `TSHELL_SSH_TEST_HOST`。已完成协议与 loopback SSH fixture，未将其冒充真实 Engine/SFTP/tmux 环境验收。

2026-10-10 主机联动调整：Docker 共用左侧主机选择器，删除独立 Engine picker 和 `docker_target` 偏好。页隐藏时只更新目标，打开时才创建所需控件和连接；Docker 页切主机不启动 shell/tmux，同一主机内切/关闭会话不会切 Engine。原生回归覆盖显示/隐藏时的本地与 SSH 主机切换、各 Engine 的容器/详情页/过滤/日志状态恢复及原有断开行为。`cargo test --workspace --locked` 仍为 221 通过、20 忽略，release 编译检查和格式检查通过；本轮报告在 `artifacts/docker-host-follow-ui.json`，截图在 `artifacts/docker-host-follow-screenshots/`，均不提交。

| 必须验证 | 场景 |
| --- | --- |
| 既有功能 | SSH/跳板/交互认证、tmux 服务端所有权、转发、指标、全局传输 |
| 工作区 | 各会话目录与未保存内容恢复、Git 打开原来源文件、主题/语言/焦点即时更新 |
| Docker 主机范围 | 本地/SSH 主机切换、隐藏页面后切主机、各 Engine 状态恢复、无终端会话、同主机内切/关闭会话 |
| 异步与 IO | 原目标确认、旧结果迟到、配置变更、stderr 隔离、取消、连接寿命与内存限额 |
| 容器终端 | 原 Engine 固定、TTY 输入输出、resize、关闭语义与 Linux/Windows 命令 |
| 界面 | 深色/浅色、1320 x 840 与 860 x 520、侧栏折叠、预览子窗口、原生选择/复制/滚动 |

测试覆盖实际行为和边界，不为搬文件、转发方法或 getter 添加镜像测试。涉及终端输入/输出/选择/Unicode 时先读固定 xterm.js 6.0.0 兼容文档并执行相关回归。真实 SSH/Docker 测试使用显式测试主机与隔离容器；缺环境的验证保留为未完成，不用 mock 替代真实远端验收。

完成标准：左侧导航清晰；会话与 Engine 独立；共享的是实际能力；工具能独立维护；Bollard 能提供的直接调用；额外代码集中在应用状态、UI 和必要 SSH connector。

## 8. 实施依据

- 仓库规则：[AGENTS.md](AGENTS.md)。
- 既有行为：[workspace](docs/workspace-interface.md)、[Git](docs/git-workspace.md)、[SFTP](docs/sftp-transfers.md)、[xterm compatibility](docs/xterm-compatibility.md)。
- 库接口：[Bollard 0.21.1](https://docs.rs/bollard/0.21.1/bollard/struct.Docker.html)。
- 固定版本实现：[docker.rs](https://github.com/fussybeaver/bollard/blob/v0.21.1/src/docker.rs)、[ssh.rs](https://github.com/fussybeaver/bollard/blob/v0.21.1/src/ssh.rs)。
- SSH 依赖限制：[openssh 0.11.5](https://docs.rs/openssh/0.11.5/openssh/)。
- Engine 与 Compose：[Docker SDK](https://docs.docker.com/engine/api/sdk/)、[Compose CLI](https://docs.docker.com/reference/cli/docker/compose/)。

## 9. Docker 资源界面与管理功能

2026-10-10 已实现，详细行为及限制见 [资源工作区设计](docs/docker-resource-design.md) 和 [Docker workspace](docs/docker-workspace.md)。

- [x] 删除重复标题、全部/运行中/已停止筛选及侧栏容器数量说明。
- [x] 接入容器 / Compose / 镜像 / 更多四个页签，按 HostId + endpoint 保留浏览状态。
- [x] 完成搜索、虚拟列表、键盘选择、原生预览/编辑器、操作菜单与右键菜单。
- [x] 完成容器平铺、概览/日志/inspect、一次资源采样与关联镜像/Volume/Network 跳转。
- [x] 完成镜像详情/history、匿名拉取、tag、CLI push、删除、归档导入/导出与创建容器。
- [x] 完成 Engine 信息、存储、Volume/Network、事件、网络连接与分类清理。
- [x] 完成创建、暂停/恢复、重命名、运行参数更新；重建保留旧容器，创建已停止替代容器，失败尝试恢复名称。
- [x] 使用共享容器 labels 和 Compose CLI 发现项目，支持文件导入/关联、已停止项目与 service 范围。
- [x] 完成有序多文件 YAML 编辑、官方校验与绑定原 Engine 的 Compose 操作。
- [x] 完成 Linux/rootless、Windows 服务、自定义配置来源探测和本地 Desktop 入口；无法核实来源时显式关联文件。
- [x] 完成 daemon JSON 编辑、校验范围说明、冲突拒绝、备份、替换写入、恢复与确认放弃草稿；离线也可修复文件。
- [x] 将保存与 reload/restart 分开，按已核实服务提供动作，等待原 Engine ID 恢复；使用当前账号权限，权限错误明确反馈。
- [x] 操作固定原目标，保留有界输出和各文件草稿；阻止带草稿/操作的主机连接变更，拒绝迟到响应。
- [x] 同步中英文 README、本地化、行为文档与原生 UI 回归。
- [ ] 在真实本地/SSH Docker 和 Compose 上验证资源操作、凭据、缺 CLI、外部变更与断线结果。
- [ ] 在真实 Desktop、Native Linux/rootless、Windows Engine 上验证配置探测、写权限、校验和服务恢复；自动提权不在本轮范围。

自动化及原生 UI 检查使用隔离数据目录和独立可执行文件；未覆盖用户运行程序或操作真实 Docker daemon。验收记录见本节后续验证条目。

本轮验证（Windows x64，2026-10-10）：

- `cargo fmt --all -- --check`、`git diff --check` 通过。
- `cargo test --workspace --locked`：233 通过、20 忽略；忽略项需要显式远端环境，不重复计算测试内启动的子进程。
- `cargo check --workspace --all-targets --locked --features ui-check-screenshots` 与独立目录 release 编译检查通过。
- 原生 `--workspace-ui-check` 通过：四类资源、10,000 容器/镜像、YAML/JSON/输出、深浅与窄窗口、键盘选择、按主机恢复、未保存草稿、迟到响应和无终端会话。截图已人工检查。
- 使用隔离 `TSHELL_DATA_DIR` 与 `target/module-check/debug/tshell.exe`，保留用户运行程序及剪贴板；未执行真实 daemon 修改。
- 报告：`artifacts/docker-resource-ui.json`；画面：`artifacts/docker-resource-screenshots/`；测试及编译日志：`tmp/modularization/resource-complete-*.log`。生成产物不提交。


## 10. Docker 主区空间与浏览交互

2026-10-10 已实现：主区默认以表格浏览容器、Compose 和镜像；更多页的 Volume / Network 分组打开对应表格。按实际主区尺寸并排或上下显示详情，可收起、放大和返回。日志、YAML、JSON 与输出使用完整主区。没有新增标题、统计卡片或侧栏数量文案。

- [x] 将比较字段和资源操作放入固定行高的虚拟表格；窄表格保留核心列，长值保留全文 tooltip。
- [x] 搜索、创建和刷新只出现一次；浏览列表时放主区，完整预览时放侧栏。
- [x] 完成方向键、Home / End、Enter、Escape，以及各主机表格/滚动/详情状态恢复。
- [x] 重复选择同一容器保留已有详情；暂停容器主操作显示恢复，暂停期间禁用终端入口。
- [x] 更新中英文 README、资源设计与工作区行为文档。
- [x] 全 workspace 测试 233 通过、20 忽略；格式、all-targets 和 release 编译检查通过。
- [x] 原生深/浅与窄窗口回归覆盖五类表格、万条键盘选择、收起/放大/返回和主机切换；实际截图检查通过。

报告：`artifacts/docker-browser-ui.json`；截图：`artifacts/docker-browser-screenshots/`；日志：`tmp/modularization/docker-browser-*.log`。生成产物不提交。真实 Engine/SSH 操作验收仍沿用第 9 节未完成项。
