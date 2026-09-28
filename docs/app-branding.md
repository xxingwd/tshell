# TShell 应用标识

`assets/tshell.svg` 是 Logo 源文件；字形使用路径，不依赖本机字体。深色圆角底、浅色 T、青绿色 S。
`tools/generate-icons.py` 使用 resvg-py 和 Pillow 导出 256px PNG 和包含 16–256px 尺寸的 ICO。
Windows 构建将 ICO 嵌入资源 1，供 GPUI 窗口和资源管理器使用。
工作区左上角只显示 TShell 文字，不显示图标：仅 S 使用 Logo 的青绿色 `#64DFC3`，T 和 hell 沿用界面正文颜色，各部分无额外间距。
名称保留窗口拖动区域，并随侧栏收起而隐藏。终端标题右侧不放置图标。

Windows 通知使用独立 AUMID `TShell.Desktop`。首次发送时将内嵌 PNG 写入
`%LOCALAPPDATA%/TShell/branding/tshell.png`，并在当前用户
`Software\Classes\AppUserModelId\TShell.Desktop` 注册 DisplayName、IconUri 和 IconBackgroundColor。
不需要管理员权限，复制 exe 后也不依赖源码目录。注册失败会记录错误，不借用 PowerShell 身份。

此前仅设置 notify-rust 的 appname；该库的 Windows 实现在缺少 app_id 时使用
`Toast::POWERSHELL_APP_ID`，因此通知会显示 PowerShell 的名称和图标。
现在仍使用 Windows 原生通知布局，只更换来源名称和图标。尚未实现点击通知跳回对应终端。
卸载时可移除以上专属注册表项和 branding 目录。已有通知不会改名，需使用新构建发送新通知。

注册方式参考锁定依赖 tauri-winrt-notification 0.7.3 的 `examples/unpackaged_app.rs`。
手工验证：`cargo test notifications_native_smoke -- --ignored --nocapture`；
应看到来源 TShell 与 TS 图标。此检查会发送一条真实桌面通知。
