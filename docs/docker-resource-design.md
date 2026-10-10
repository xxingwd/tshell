# Docker 资源工作区设计

更新日期：2026-10-10。状态：设计与下一阶段实施计划；当前代码仍是容器工作区。

## 布局

Docker 继续跟随工作区当前主机，独立于终端会话。左侧复用已有主机选择器和 Terminal / Explorer / Git / Docker 切换栏。

Docker 内容区使用四个固定页签：**容器 / Compose / 镜像 / 更多**。页签在侧栏顶部，下面是搜索与当前资源列表；窄侧栏使用图标与 tooltip，宽度足够时显示短标签。页签顺序、选中状态、键盘访问和列表行高固定。

删除侧栏额外的 Docker 标题、全部/运行中/已停止按钮，以及“6 个容器”一类统计文案。运行状态通过资源行的状态图标和 tooltip 表达，搜索按当前资源类型进行。

主区展示当前资源的预览或编辑器。窗口现有标题承担主机/Engine 来源展示，删除主区重复的 Engine 标题栏。主区仅有一条资源栏，放当前资源名称、适用的详情页签和常用操作；复杂操作收进菜单。概览、日志、配置是所选资源的详情页签，与侧栏资源类型页签分开。

```text
┌────────────────────────现有窗口标题与窗口控件───────────────────────────┐
│ 当前主机 ▾             │ 当前资源名称   概览 / 日志 / 配置     操作 …    │
│ Terminal Explorer Git Docker│                                         │
│ 容器 Compose 镜像 更多 │                                               │
│ 搜索当前资源       ↻ + │           当前资源预览 / 编辑器                │
│                       │                                               │
│ 当前资源列表          │           关联资源可点击跳转                    │
│                       │                                               │
└───────────────────────┴───────────────────────────────────────────────┘
```

刷新作用于当前资源页。创建按钮只有当前页支持创建时出现；没有选择资源时，主区提供对应的创建/导入入口，不默认弹出一个概览仪表盘。加载、空列表、连接失败和权限失败各自有明确反馈。

## 四类资源与操作

| 页签 | 侧栏 | 主区预览 | 集成操作 |
| --- | --- | --- | --- |
| 容器 | 平铺容器，名称与运行状态；不再嵌入 Compose 分组 | 概览、日志、资源使用、配置；镜像、端口、挂载和网络可跳转 | 创建、启动、停止、重启、暂停/恢复、终端、重命名、删除；支持的运行参数更新与重建分开 |
| Compose | 项目 → service；项目列表包含已停止项目，并可导入配置文件 | 项目概览、服务状态、服务日志、Compose 文件和命令结果 | 校验配置、up、start/stop、restart、pull、build、down；允许选择 service |
| 镜像 | repository:tag；无 tag 镜像用短 ID，选择绑定完整 ID | 标签、大小、创建时间、层/history、引用容器、原始配置 | 拉取、打 tag、push、删除、导入/导出；从镜像创建容器 |
| 更多 | Engine 信息、存储用量、Volumes、Networks、事件、Engine 配置 | 所选资源的结构化详情与原始配置；Engine 配置可编辑 | Volume 创建/删除；Network 创建/删除与连接/断开容器；分类清理；配置校验、保存、重载/重启 |

“更多”是一个可扩展的分组资源列表，不把 Volume、Network 等继续加成顶层页签。Build cache 等后续能力加入对应分组，保持四个顶层入口稳定。

同一种操作放在相同位置：主区资源栏提供常用动作，右键菜单补充完整动作，创建/导入入口放当前资源页的搜索栏。命令输出、拉取/推送进度和失败原因展示在所属资源的预览中，不在侧栏堆说明文字。删除与 down 明确说明目标，删除 Volume、附带删除数据和强制操作不作为默认选项。

资源之间的跳转仍发生在 Docker 内：容器 → 镜像、挂载 → Volume、网络 → Network、Compose service → 容器。跳转保留原资源页的搜索、选择和滚动位置。

## Compose 的实现边界

Engine API 的容器 labels 可以识别 Compose project/service，先以共享容器快照建立项目视图；没有 Compose CLI 时仍能浏览已有项目、容器和日志。官方规范定义了 `com.docker.compose.project` 与 `com.docker.compose.service`，不能把任意名称相似的容器猜成一个项目。

完整部署语义使用目标主机上的 **Docker Compose CLI**，复用 `tshell-host::CommandRequest`；不在 Rust 中重新实现 Compose YAML 的 merge、profiles、env 插值、依赖和 build 语义。项目发现可结合 `docker compose ls --all --format json`；用户导入的配置也纳入列表，不依赖已运行容器才能发现项目。

项目绑定明确的 project name、配置文件集合、所属主机和工作目录。labels 中的配置路径仅作为线索，读取前确认它在该主机可访问；文件可能已删除，或来自另一个部署机器。来源不可访问时支持浏览运行状态与重新关联文件，不伪造可编辑配置。

CLI 操作必须使用与 Bollard 相同的实际 Engine endpoint/context；不能无参数调用本地 CLI，让 `DOCKER_HOST` 或当前 context 意外切换目标。SSH 环境也固定所用 endpoint/context，操作前核对 Engine 身份。参数通过已有类型化命令构造传递，project name 与文件路径不拼进未经转义的 shell 文本。

Compose 文件使用 Docker 自己的 `compose config --quiet` 校验。编辑保留源文件，而不是把展开后的配置覆盖原 YAML；多文件项目能选择编辑哪个源文件。未知环境变量、缺少 `.env`、CLI 缺失和插件版本不支持均显示在原项目预览中。

## Engine 配置编辑

入口是 **更多 → Engine 配置**。主区使用原生 JSON 编辑器，提供配置来源、校验、保存、Diff/恢复备份和明确的应用操作；不复制一套与 JSON 容易不一致的完整设置表单。

Bollard 继续处理 Engine API 请求。`daemon.json` 编辑使用主机文件与命令能力，识别实际 daemon 的安装方式与管理方式：

| 配置来源 | 默认路径线索 | 应用方式 |
| --- | --- | --- |
| Linux 系统 Engine | `/etc/docker/daemon.json` | 检测实际服务，支持的变更可 reload，其余明确 restart |
| Linux rootless | `$XDG_CONFIG_HOME/docker/daemon.json` 或 `~/.config/docker/daemon.json` | 绑定 daemon 所属用户及对应用户服务 |
| Windows 原生 Engine | `C:\ProgramData\docker\config\daemon.json` | 检测实际 Windows 服务与 daemon |
| Docker Desktop | 当前文档的 `~/.docker/daemon.json`，同时核对安装版本与实际文件 | 检测 Desktop CLI，可用时提供 `docker desktop restart`，否则提供 Desktop 设置入口 |
| 自定义安装 | daemon 启动参数 `--config-file` 或用户指定路径 | 绑定实际配置文件与经过确认的管理方式 |

以上路径是探测线索，不是对所有主机的保证。Windows 上通过 named pipe 连接到 Linux Engine 时，不按客户端操作系统选择 Windows 服务的配置文件。Docker socket 可访问也不代表有配置文件写权限。SSH 目标上的 Docker context 可能指向另一台 daemon，不能默认把 SSH 主机文件当成该 daemon 的配置。

保存流程：

1. 加载配置与文件版本，保留未知字段和该主机的编辑草稿。
2. 校验 JSON 对象；对应主机有 `dockerd` 时，对临时文件执行 `dockerd --validate --config-file <path>`，不启动一个新 daemon。缺少完整校验能力时准确标明校验范围。
3. 保存前比对文件版本，外部修改时提供冲突 Diff；备份原文件，以临时文件替换写入并保持文件权限。远端文件系统不能保证原子替换时明确处理失败与恢复。
4. 将“保存配置”和“重载/重启应用”分成明确动作。只有当前 daemon 版本与管理方式支持的字段才尝试 reload；其他变更明确需要 restart。
5. 重载/重启后重新读取 Engine 状态。失联时保留源配置、草稿、备份与操作结果，不能以请求已发送冒充已经生效，也不自动重复重启。

权限不足时显示实际错误并提供适用的提权入口；不静默假定 `sudo` 可用。提权凭据与已有 SSH 登录认证分开处理。Docker Desktop 使用公开配置方式，不盲写其内部 `settings-store.json`，不硬编码未经验证的内部控制接口。

## 代码与状态所有权

保留现有 workspace/crate 边界。根应用只提供当前 HostId、连接配置、窗口布局与终端接入；全部 Docker 资源、编辑草稿、操作状态仍属于 `tshell-docker`。

资源导航使用一个明确的枚举 `ResourcePage::{Containers, Compose, Images, More}`。每个 Engine 按 HostId 保存四个资源页各自的列表、搜索、选择、滚动和详情页；容器、镜像、Compose、Volume 和 Network 使用各自的具体类型，完整 ID/name 在对应域内定位。当前预览从资源页的选择派生，避免另存一套可能错配的全局资源选择。

保留现有 Engine、事件和操作调度逻辑，按需要拆出有实际职责的文件：

```text
crates/tshell-docker/src/
  panel.rs          # 主机范围的状态、导航、异步消息与生命周期
  view.rs           # 四类资源入口、列表与主区组合
  containers.rs     # 容器预览、创建、运行参数与生命周期操作
  images.rs         # 镜像列表、预览与操作
  compose.rs        # 项目发现、配置文件与 Compose 命令
  resources.rs      # Volume、Network、Engine 信息与分类清理
  daemon.rs         # 配置来源探测、编辑保存、校验与应用
  connection.rs     # 已有 Bollard 本地/SSH 连接
  logs.rs           # 已有有界日志缓冲
  terminal.rs       # 已有容器终端 IO
```

不为每个资源另建 crate，不再包装通用 Docker SDK，不建立资源插件注册器。原生 EditorState、命令执行、SSH/SFTP、主题和本地化复用现有能力。

容器与 Compose 共用容器快照；其他列表按页激活加载，预览按选择加载，事件只使相关资源快照失效。页隐藏时停用浏览请求、stats 与日志订阅，避免打开 Docker 就持续扫描所有资源。

异步响应绑定原始 HostId、endpoint、资源身份和请求版本。跨主机/跨页旧响应不得覆盖当前预览；已执行的操作保持原目标，返回后更新原目标缓存。配置草稿不因主机切换或页面隐藏而丢失，删除/修改主机连接时须处理相关草稿与执行中的操作。已有容器终端继续固定原 Engine。

## 实施顺序与验收

1. 先定四类资源导航与统一主区，删除重复标题、状态筛选与侧栏计数。同步本地化与窄窗口原生回归，不先用四个空页签冒充已实现。
2. 接入容器、镜像和更多资源的 Bollard 查询/预览，让资源跳转成立；容器与 Compose 共用快照。
3. 补齐对应操作、创建/导入表单、右键菜单与进度反馈；用真实变化与失败场景验证操作结果。
4. 完成 Compose 项目来源、文件编辑校验与目标绑定的 CLI 操作，验证缺 CLI 和源文件不可访问的情况。
5. 完成 daemon 配置来源探测、编辑、校验、冲突/备份/恢复与配置应用；验证 Native Linux、rootless、Windows Engine、Docker Desktop 和 SSH 上的适用能力。
6. 跑 workspace 测试与 Windows 原生 UI 回归。覆盖四类资源的深/浅主题、窄侧栏、长名称、大列表、键盘操作、主机切换、迟到响应、编辑草稿与操作结果来源。真实 Compose/daemon 操作使用指定测试主机与隔离资源；环境缺口保留为未完成。

## 官方依据

- [Engine 配置位置与自定义配置路径](https://docs.docker.com/engine/daemon/)
- [dockerd 校验与配置重载范围](https://docs.docker.com/reference/cli/dockerd/)
- [Docker Desktop 的 Docker Engine 设置](https://docs.docker.com/desktop/settings-and-maintenance/settings/#docker-engine)
- [Docker Desktop restart](https://docs.docker.com/reference/cli/docker/desktop/restart/)
- [Compose project/service labels](https://docs.docker.com/reference/compose-file/services/#labels)
- [Compose 项目列表](https://docs.docker.com/reference/cli/docker/compose/ls/)
- [Compose project name](https://docs.docker.com/compose/how-tos/project-name/)
