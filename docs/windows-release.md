# Windows 自动发布

源码和二进制都放在公开 GitHub 仓库 `xxingwd/tshell`。
只在推送 `v*` tag 时运行 CI；普通提交不会触发构建。
GitHub Actions 自动测试、编译 Windows x64、打包、生成更新信息，并发布到**同仓库 Release**。
使用 GitHub 自动提供的 `GITHUB_TOKEN`，不需要配置 Secret、PAT 或签名密钥，也不需要本地打包。

## 发布

1. 将源码和 `.github/workflows/windows.yml` 推送到公开 GitHub 仓库，确保 Actions 已启用。
2. 修改 `Cargo.toml` 的版本号，运行 `cargo metadata --no-deps --format-version 1` 更新 Cargo.lock，提交修改。
3. 推送匹配的 tag，例如 Cargo 版本 `0.1.0` 对应 `v0.1.0`：

```powershell
git tag v0.1.0
git push <github-remote> v0.1.0
```

如果还没添加 GitHub remote，可以执行：

```powershell
git remote add github https://github.com/xxingwd/tshell.git
git push github HEAD
git push github v0.1.0
```

CI 的 token 写权限由工作流的 `permissions: contents: write` 声明。
如果组织策略禁止写入，需要在 GitHub 设置中允许 Actions 创建 Release。
tag 必须与 Cargo 版本一致，且版本递增；暂不支持 beta 等预发布通道。
附件全部上传成功后，草稿才公开并设为 latest。已有同名 Release 不会被覆盖；失败残留的草稿需检查后再重试。

## 产物

- `tshell-windows-x86_64.zip`：包含 `tshell.exe`、使用说明和第三方许可。
- `tshell-windows-x86_64.exe`：内置更新下载的同一程序。
- `update-windows-x86_64.json`：版本、架构、文件名、文件大小和 SHA-256。
- `SHA256SUMS.txt`、`THIRD-PARTY-NOTICES.txt`：下载校验与许可信息。

ZIP 解压到用户可写目录后运行 `tshell.exe`。CRT 静态链接，ConPTY 已嵌入程序。
当前没有安装器、开机自启或 Authenticode 签名。正式构建由 CI 完成；本地验证可运行：

```powershell
cargo test --locked --bin tshell
cargo build --locked --release --bin tshell
./tools/package-windows.ps1
```

## 内置更新

Windows x64 release 构建默认启用；debug 构建禁用。
CI 自动将当前仓库名编译进程序，本地 release 构建默认使用 `xxingwd/tshell`。
启动 15 秒后检查，随后每 6 小时检查；设置 → 软件更新也可以手动检查。

通过 HTTPS 从公开 Release 下载，只有版本更新、架构匹配、文件大小和 SHA-256 一致才接受。
SHA-256 用于检测文件损坏，不提供独立签名验证；更新信任 GitHub 仓库及 HTTPS。
客户端不携带任何 GitHub token。

下载在后台进行，下次启动时安装，再次校验后替换程序。
不会强制重启正在使用的终端；“重启并更新”会提示连接断开，未保存文件或正在写文件时阻止重启。
有其他 TShell 实例运行时推迟安装。配置文件不参与程序替换。

安装保留 `tshell.previous.exe`，创建新进程失败时恢复旧版；不包含新版本启动后的健康检测或崩溃回滚。
断电发生在替换过程中可能需要手动从备份恢复。
需要手动恢复时，关闭全部实例，将备份恢复为 `tshell.exe`，并删除对应更新缓存中的 `pending.json`。
缓存位于系统 cache 目录的 `tshell/updates/<安装路径哈希>/`。

本机 PTY 和普通 SSH 无法跨重启保活；远端 tmux 由服务器保留。
网络失败继续使用当前版本，设置页显示错误并允许重试。
