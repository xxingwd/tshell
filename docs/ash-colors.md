# ash 在白天主题下的颜色

检查版本：[xxingwd/ash 055742b8b589bfec151ae0d0a71bfbce5ab93ad0](https://github.com/xxingwd/ash/tree/055742b8b589bfec151ae0d0a71bfbce5ab93ad0)，2026-09-15 检查。

ash-tui 使用 Ratatui 0.30.2 / Crossterm 0.29。源码没有 OSC 10/11、CSI 996/2031、COLORFGBG 等明暗主题探测逻辑；大部分文字使用默认前景，语法高亮使用 ANSI 命名色。

- [live_block.rs](https://github.com/xxingwd/ash/blob/055742b8b589bfec151ae0d0a71bfbce5ab93ad0/crates/ash-tui/src/live_block.rs#L575)：欢迎卡边框使用 Cyan，字幕使用 DIM；`dimmed` 在第 934 行将工具输出的每个 span 弱化。
- [history_block.rs](https://github.com/xxingwd/ash/blob/055742b8b589bfec151ae0d0a71bfbce5ab93ad0/crates/ash-tui/src/history_block.rs#L87)：用户提示符使用 BOLD + DIM，正文使用默认色。
- [markdown.rs](https://github.com/xxingwd/ash/blob/055742b8b589bfec151ae0d0a71bfbce5ab93ad0/crates/ash-tui/src/markdown.rs#L733)：代码语言标签使用 Cyan + DIM。
- [ansi.rs](https://github.com/xxingwd/ash/blob/055742b8b589bfec151ae0d0a71bfbce5ab93ad0/crates/ash-tui/src/ansi.rs#L21)：工具输出保留 ANSI 色，命令高亮使用 Magenta/Green/Cyan/DarkGray/Blue。

TShell 原先将 DIM 前景的每个 RGB 通道乘以 2/3。这在暗色背景上降低对比度，却在亮色背景上增加对比度。例如白天默认前景 `#273244` 被变为 `#1a212d`，弱化文字反而更黑。

[xterm.js WebGL TextureAtlas](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-webgl/src/TextureAtlas.ts#L350) 对 DIM 使用 [0.5 的文字不透明度](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-webgl/src/Constants.ts#L8)。TShell 现在保留前景 RGB，绘制时使用 50% 文字不透明度参与实际背景合成：上述前景在白底上约为 `#9399a2`。没有提前将文字与固定背景预混合，因此透明背景变化后也能重新合成。选区自身的前景覆盖规则尚未完成 xterm.js 对照。

反色先交换前景背景，再按粗体规则将显示前景的 ANSI 0–7 映射到亮色 8–15，最后应用 SGR 2。弱化覆盖真彩色、默认下划线、删除线与自绘边框；显式 SGR 58 下划线颜色按 xterm.js 保持不透明。OSC 应用颜色覆盖照常生效。ANSI 调色板与 ash 源码没有修改，也没有向 ash 注入未经订阅的主题通知。后续行为统一遵循 [xterm.js 兼容约定](xterm-compatibility.md)。

验证覆盖 ash 使用的样式组合、SGR 22 恢复、两套主题、反色、真彩色和 OSC 颜色覆盖，并在 GPUI 隐藏窗口中绘制样例。测试使用相同的终端控制序列，没有运行 ash 的模型请求，不能据此保证其所有外部工具输出的配色。

## 蓝色与透明背景的进一步排查

已核对 ash 锁定的 `ratatui-crossterm 0.1.2` 和 `crossterm 0.29.0` 发布源码：Ratatui `Color::Blue` 映射为 Crossterm `DarkBlue`，输出 `SGR 38;5;4`；它与 shell 常用的 `SGR 34` 都选择调色板第 4 色。新增测试覆盖两种编码、粗体、DIM、恢复普通强度、OSC 覆盖和主题切换。未取得用户当前异常位置的原始输出，因此不能断言该位置仅包含普通 Blue。

进一步确认明暗模式蓝色深浅不同来自两套 ANSI 调色板。现已提供独立的
终端主题选择，默认 VS Code Dark Modern；切换界面
明暗不会改变终端配色。每套主题使用已提交的 RGB，包括前景、背景、ANSI 色、
光标及选区。用户可直接选择偏好的深浅配色，xterm.js 不规定主题必须使用哪一种蓝色。

另一个已复现的问题位于 Windows GPUI 的 DirectX 混合状态：背景和文字的 alpha 被相加，RGB 却使用 source-over，导致透明窗口最终合成时额外发暗。已在本地依赖中修复，并通过 80 组 GPU 像素回读验证；详见 [补丁说明](../vendor/gpui-pre-windows/TSHELL_PATCH.md)。此前颜色解析测试没有覆盖这一层，因此即使前景 RGB/alpha 测试通过，屏幕仍可能偏暗。正确合成后，DIM 文字仍会按 xterm.js 的 50% ink alpha 与桌面背景混合，不能保证调节透明度后其最终颜色完全不变。
