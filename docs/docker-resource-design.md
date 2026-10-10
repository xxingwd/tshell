# Docker 资源工作区设计

更新日期：2026-10-10。状态：功能菜单、主区列表和详情已实现；aidev 的只读列表已核对，资源写操作与配置恢复仍待真实环境验收。实际要求和限制以 [Docker workspace](docker-workspace.md) 为准。

## 布局

Docker 继续跟随工作区当前主机，独立于终端会话。左侧复用已有主机选择器和 Terminal / Explorer / Git / Docker 切换栏。

点击 Docker 后，现有左栏下半部分变成功能菜单：容器、Compose、镜像、Volumes、Networks、Engine 信息、存储、事件、Engine 配置。使用图标和短标签显示当前位置，可通过方向键和 Home / End 切换。不增加另一条导航侧栏，也不在左栏重复列出资源。

主区默认显示所选功能的资源表格或页面。搜索、创建和刷新在表格工具栏中；搜索可清空。现有窗口标题承担主机来源展示，不增加 Docker 标题、汇总卡片、状态筛选或侧栏资源数量描述。

选中资源后保留表格并打开详情。按主区实际像素宽度布局：达到 960 px 时列表和详情左右并排，较窄时上下排列。可收起详情继续浏览，或放大详情使用整块主区；返回按钮恢复该资源类型的列表。日志、YAML、JSON 和操作输出使用完整主区。各主机保留表格、选择、滚动和详情状态，切主机不创建终端会话。

表格固定行高、虚拟化，名称和长路径截断后以 tooltip 展示全文。窄表格保留核心列；容器状态始终可见。右侧操作菜单和右键菜单绑定完整资源 ID。表格支持方向键、Home / End 选择，Enter 放大详情，Escape 收起详情。操作继续使用已有原目标绑定、状态校验和确认流程。

刷新作用于当前资源页。创建和导入从当前浏览区域完成。连接与权限失败在主区反馈，加载时保持列表与列头的位置稳定。

## 列表与身份

容器请求 `all=true`，搜索为空时显示当前 Engine 的全部容器，包括停止、暂停、Compose 管理的容器；与当前终端会话无关。Compose 页面复用这份完整快照分组。

镜像请求使用普通范围 `all=false`，不展开所有中间对象。每个完整 `仓库:标签` 单独成行；同一 ID 的多个标签保持独立选择。有标签项按完整标签排序，无标签项显示 `<none>:<none>`，按完整 ID 排在底部。列表有独立的短 ID 列，tooltip 与操作保留完整 ID；大小以十进制 MB/GB 显示。镜像详情缓存按 ID 共享，标签选择不合并。

删除有标签项只移除所选标签，执行前重新 inspect 并核对完整 ID；标签已指向其他镜像时拒绝操作。无标签项按 ID 删除，保留已有非强制删除规则。Push 表单预填所选标签，其余操作继续绑定原 Engine 与完整 ID。

2026-10-10 只读核对 aidev：`docker ps -a` 和 Engine API 都返回 1 个容器；默认镜像 API 展开出的 52 条有标签记录与 CLI 的 `(完整标签, 完整 ID)` 集合一致。CLI 默认会隐藏无标签项，应用按本次需求把 API 返回的无标签项保留在底部。此次未在该主机执行写操作或 daemon 修改。

## 资源与操作

| 功能 | 导航/数据 | 主区预览 | 集成操作 |
| --- | --- | --- | --- |
| 容器 | 当前 Engine 全部容器 | 概览、日志、资源使用、配置；镜像、端口、挂载和网络可跳转 | 创建、启动、停止、重启、暂停/恢复、终端、重命名、删除；支持的运行参数更新与重建分开 |
| Compose | 项目表格；包含已停止项目，可导入配置文件 | 项目概览、服务状态、服务日志、Compose 文件和命令结果 | 校验配置、up、start/stop、restart、pull、build、down；允许选择 service |
| 镜像 | repository:tag；无 tag 镜像用短 ID，选择绑定完整 ID | 标签、大小、创建时间、层/history、引用容器、原始配置 | 拉取、打 tag、push、删除、导入/导出；从镜像创建容器 |
| 更多 | Engine 信息、存储用量、Volumes、Networks、事件、Engine 配置 | 所选资源的结构化详情与原始配置；Engine 配置可编辑 | Volume 创建/删除；Network 创建/删除与连接/断开容器；分类清理；配置校验、保存、重载/重启 |

“更多”是一个可扩展的分组资源列表，不把 Volume、Network 等继续加成顶层页签。Build cache 等后续能力加入对应分组，保持四个顶层入口稳定。

同一种操作放在相同位置：容器资源栏提供常用动作，其他资源通过同位置的操作菜单和右键菜单访问动作，创建/导入入口放当前资源页的搜索栏。命令输出、SDK 拉取/归档进度和失败原因展示在所属资源的预览中；CLI 输出在命令结束后显示，不在侧栏堆说明文字。删除与 down 明确说明目标，删除 Volume、附带删除数据和强制操作不作为默认选项。

资源之间的跳转仍发生在 Docker 内：容器 → 镜像、挂载 → Volume、网络 → Network、Compose service → 容器。跳转保留原资源页的搜索、选择和滚动位置。

## Compose 的实现边界

Engine API 的容器 labels 可以识别 Compose project/service，先以共享容器快照建立项目视图；没有 Compose CLI 时仍能浏览已有项目、容器和日志。官方规范定义了 `com.docker.compose.project` 与 `com.docker.compose.service`，不能把任意名称相似的容器猜成一个项目。

完整部署语义使用目标主机上的 **Docker Compose CLI**，复用 `tshell-host::CommandRequest`；不在 Rust 中重新实现 Compose YAML 的 merge、profiles、env 插值、依赖和 build 语义。项目发现可结合 `docker compose ls --all --format json`；用户导入的配置也纳入列表，不依赖已运行容器才能发现项目。

项目绑定明确的 project name、配置文件集合、所属主机和工作目录。labels 中的配置路径仅作为线索，读取前确认它在该主机可访问；文件可能已删除，或来自另一个部署机器。来源不可访问时支持浏览运行状态与重新关联文件，不伪造可编辑配置。

CLI 操作必须使用与 Bollard 相同的实际 Engine endpoint/context；不能无参数调用本地 CLI，让 `DOCKER_HOST` 或当前 context 意外切换目标。SSH 环境也固定所用 endpoint/context，操作前核对 Engine 身份。参数通过已有类型化命令构造传递，project name 与文件路径不拼进未经转义的 shell 文本。

Compose 文件使用 Docker 自己的 `compose config --quiet` 校验。编辑保留源文件，而不是把展开后的配置覆盖原 YAML；多文件项目能选择编辑哪个源文件。未知环境变量、缺少 `.env`、CLI 缺失和插件版本不支持均显示在原项目预览中。

## Engine 配置编辑

入口是 **更多 → Engine 配置**。主区使用原生 JSON 编辑器，提供配置来源、校验、保存、恢复备份和明确的应用操作；不复制一套与 JSON 容易不一致的完整设置表单。

Bollard 继续处理 Engine API 请求。`daemon.json` 编辑使用主机文件与命令能力，识别实际 daemon 的安装方式与管理方式：

| 配置来源 | 默认路径线索 | 应用方式 |
| --- | --- | --- |
| Linux 系统 Engine | `/etc/docker/daemon.json` | 检测实际服务，支持的变更可 reload，其余明确 restart |
| Linux rootless | `$XDG_CONFIG_HOME/docker/daemon.json` 或 `~/.config/docker/daemon.json` | 绑定 daemon 所属用户及对应用户服务 |
| Windows 原生 Engine | `C:\ProgramData\docker\config\daemon.json` 或服务 `--config-file` | 检测默认管道及 Windows docker 服务 |
| Docker Desktop | 本地 `~/.docker/daemon.json` | 明确 `docker desktop restart`，缺 CLI 时显示错误 |
| 自定义安装 | daemon 启动参数 `--config-file` 或用户指定路径 | 允许显式关联文件；无法核实管理方式时只提供文件操作 |

以上路径是探测线索，不是对所有主机的保证。Windows 上通过 named pipe 连接到 Linux Engine 时，不按客户端操作系统选择 Windows 服务的配置文件。Docker socket 可访问也不代表有配置文件写权限。SSH 目标上的 Docker context 可能指向另一台 daemon，不能默认把 SSH 主机文件当成该 daemon 的配置。

保存流程：

1. 加载配置与文件版本，保留未知字段和该主机的编辑草稿。
2. 校验 JSON 对象；对应主机有 `dockerd` 时，对临时文件执行 `dockerd --validate --config-file <path>`，不启动一个新 daemon。缺少完整校验能力时准确标明校验范围。
3. 保存前比对文件版本，外部修改时拒绝覆盖并显示冲突；备份原文件，以临时文件替换写入并保持文件权限。远端文件系统不能保证原子替换时明确处理失败与恢复。
4. 将“保存配置”和“重载/重启应用”分成明确动作。仅在服务报告支持 reload 时显示入口，并提示 Docker 只能重载支持的选项；其他变更需 restart。
5. 重载/重启后重新读取 Engine 状态。失联时保留源配置、草稿、备份与操作结果，不能以请求已发送冒充已经生效，也不自动重复重启。

权限不足时显示实际错误，文件和服务操作使用当前本地/SSH 账号；本轮没有自动 sudo、Windows 提权或额外凭据流程。离线时可关联/修复配置文件，服务启动由外部处理；在线 reload/restart 才能核实 Engine 绑定。Docker Desktop 使用公开配置方式，不盲写其内部 `settings-store.json`，不硬编码未经验证的内部控制接口。

## 代码与状态所有权

保留现有 workspace/crate 边界。根应用只提供当前 HostId、连接配置、窗口布局与终端接入；全部 Docker 资源、编辑草稿、操作状态仍属于 `tshell-docker`。

资源导航使用一个明确的枚举 `ResourcePage::{Containers, Compose, Images, More}`。每个 Engine 按 HostId 保存四个资源页各自的列表、搜索、选择、滚动和详情页；容器、镜像、Compose、Volume 和 Network 使用各自的具体类型，完整 ID/name 在对应域内定位。当前预览从资源页的选择派生，避免另存一套可能错配的全局资源选择。

保留现有 Engine、事件和操作调度逻辑，按需要拆出有实际职责的文件：

```text
crates/tshell-docker/src/
  panel.rs          # 主机状态、异步消息、表单与生命周期
  view.rs           # 容器详情、日志与现有 exec 入口
  resource_view.rs  # 四类侧栏、预览、菜单与资源跳转
  resources.rs      # 资源身份、SDK 查询和按页浏览状态
  actions.rs        # 类型化操作、SDK 调用与文件/CLI 边界
  images.rs         # 本地镜像归档流
  compose.rs        # 项目发现、多文件校验与 Compose 命令
  daemon.rs         # 配置来源、校验与服务控制
  documents.rs      # 文件版本、草稿、冲突检测与备份保存
  cli.rs            # 明确 endpoint/context、Engine 身份核对
  connection.rs     # 已有 Bollard 本地/原生 SSH transport
  logs.rs           # 已有有界日志缓冲
  terminal.rs       # 已有容器终端 IO
```

不为每个资源另建 crate，不再包装通用 Docker SDK，不建立资源插件注册器。原生 EditorState、命令执行、SSH/SFTP、主题和本地化复用现有能力。

容器与 Compose 共用容器快照；其他列表按页激活加载，预览按选择加载，事件只使相关资源快照失效。页隐藏时停用浏览请求、stats 与日志订阅，避免打开 Docker 就持续扫描所有资源。stats 是选择容器后的一次采样；事件列表限制 500 条。

异步响应绑定原始 HostId、endpoint、资源身份和请求版本。跨主机/跨页旧响应不得覆盖当前预览；已执行的操作保持原目标，返回后更新原目标缓存。配置草稿不因主机切换或页面隐藏而丢失；放弃修改需要确认，并只复原本地草稿，不写入文件，删除/修改主机连接时须处理相关草稿与执行中的操作。已有容器终端继续固定原 Engine。

## 实施记录与验收

- 已完成四类资源导航、原生预览/YAML/JSON 编辑、关联跳转、资源表单、右键操作和原目标结果。
- 已完成 Compose 有序多文件编辑、官方校验、service 范围和原 Engine CLI 绑定。
- 已完成配置来源探测、离线文件修复、冲突检测、备份恢复及明确的服务 reload/restart；应用后等待原 Engine ID 恢复，不能由此推断所有字段已生效。
- 重建容器保留旧容器备份，并创建已停止的替代容器；不默认清理 Volume。拉取暂为匿名 SDK，push 使用主机 CLI 凭据；导入项目和编辑草稿保留在本次应用生命周期。
- 自动化覆盖深/浅主题、窄侧栏、万容器/镜像列表、键盘、主机切换、迟到响应、草稿和结果。生成报告/截图不提交。
- 真实本地/SSH Docker、Compose 插件、Desktop/rootless/Windows 服务、权限与断线恢复仍需指定环境验收；不将协议与 UI fixture 当作真实部署验证。

## 官方依据

- [Engine 配置位置与自定义配置路径](https://docs.docker.com/engine/daemon/)
- [dockerd 校验与配置重载范围](https://docs.docker.com/reference/cli/dockerd/)
- [Docker Desktop 的 Docker Engine 设置](https://docs.docker.com/desktop/settings-and-maintenance/settings/#docker-engine)
- [Docker Desktop restart](https://docs.docker.com/reference/cli/docker/desktop/restart/)
- [Compose project/service labels](https://docs.docker.com/reference/compose-file/services/#labels)
- [Compose 项目列表](https://docs.docker.com/reference/cli/docker/compose/ls/)
- [Compose project name](https://docs.docker.com/compose/how-tos/project-name/)
