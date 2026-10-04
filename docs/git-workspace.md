# Git workspace

- The default view is “仅看变更”: changes plus three context lines on each side, with overlapping context windows merged. Hidden runs appear as clickable `···`; clicking expands that run. A separate button switches to “完整文件”, independently of inline/side-by-side layout. Both projections are computed once from the already loaded full-context patch, so toggling or expanding does not run Git again. The setting lasts for the current app session; expansions belong to the current file and layout.

- The sidebar uses Explorer's 30px rows with explicit full-width hit areas and selection backgrounds: status letter, filename, subdued relative directory, and right-aligned red removals / green additions. Binary counts use `—`.
- The list first loads only porcelain status records. A separate background operation loads numstat counts and updates the existing rows. Untracked counts use batches of 32 files (one SSH round trip per batch); unknown or pending counts use `—`. Counts never request patch text.
- Clicking a file runs a separate `--patch --unified=2147483647` command and opens a read-only, side-by-side comparison inspired by [VS Code's diff editor](https://code.visualstudio.com/docs/sourcecontrol/overview). HEAD is on the left; the working tree is on the right. Replacement lines are paired, with empty cells for unmatched additions/deletions. Both panes share vertical scrolling and have independent horizontal scrolling. Raw patch headers are hidden; text changes include the complete unchanged context; any remaining separated hunks use only an ellipsis. Binary and rename-only changes have concise empty states. Both the file list and comparison rows remain virtualized; widths and row pairing are computed once in the background. “打开文件” opens the normal editor with a loading label and retains its unsaved-change prompt. Returning to the same already-open file preserves unsaved edits. Remote Git paths keep POSIX separators so SFTP can open them on Windows.
- Segmented toolbar controls choose inline/side-by-side layout and changed/full context without reloading the patch. The layout preference is retained for other files in the current app session; switching layouts resets scroll offsets. Inline rows preserve patch order and show both old and new line numbers. Both nested scroll containers restrict input to their own axes, so vertical wheel events cannot turn into horizontal drift.
- Both layouts support native mouse selection, double-click words, triple-click lines, and copying with `Ctrl+C`, the configured Copy shortcut, or the toolbar icon. Each comparison pane has its own source document; line numbers and patch markers are excluded, tabs and Unicode are preserved, and selection retains its anchor as rows leave the virtual list. Collapsed context does not remove underlying source text from a selection spanning that context.
- Replacement lines are paired by position and compared by character. Changed character spans receive a stronger background inside the existing line tint. Per-line comparison has a 10 ms budget and skips lines over 16,384 bytes, retaining line-level highlighting for those cases. This does not provide syntax highlighting or semantic word matching.
- Counts and patches compare HEAD with the working tree, including staged and unstaged changes together. Unborn repositories use the empty tree; untracked files compare against an empty file. Rename sources are retained. Refresh clears the previous comparison; request IDs reject stale asynchronous results.
- Local and SSH workspaces share Git argument construction and output parsing. SSH requires a POSIX shell, as with the existing remote Git support.
- This is a read-only diff viewer. Syntax highlighting, staging hunks and image diffs remain unsupported. Git output over 20 MiB is rejected. Untracked-file counts still require one Git process per file, but run after the list appears and are batched over SSH.

Validation includes real temporary-repository tests (unborn, staged plus unstaged, rename, deletion, binary and empty files), NUL-delimited numstat parsing, hunk line numbers, and an isolated GPUI draw check with 10,000 files and 10,000 diff lines. A local debug-build comparison measured three draws at 8,091 / 7,451 / 6,694 ms before virtualization and 41 / 30 / 31 ms after. This is a synthetic rendering benchmark, not a measurement of SSH latency or the user's repository. Remote execution is not covered by the local draw check.

The UI check also dispatches real vertical and horizontal wheel events in inline mode and both comparison panes. It verifies that vertical input leaves horizontal offsets unchanged, and horizontal input changes only the horizontal offset. The hidden Windows test window is resized before hit testing to initialize its native viewport.

Additional regression checks reconstruct both complete file versions from a middle-of-file edit, verify remote path separators, and exercise opening a file from Git and returning to an unsaved editor.
Native UI checks also drag-select both layouts, verify system clipboard text,
exercise word/line selection, and copy across off-screen rows. Unicode character
highlighting and raw source preservation have focused unit coverage.

Selection geometry and painting share the same GPUI `StyledText` layout, including
font fallback and horizontal scroll offsets. Row hit testing uses half-open
vertical intervals so a word or line endpoint at a row's top belongs to that row.
Clicking text focuses its comparison pane; gutters and unmatched empty cells do
not start text selection. Native checks cover adjacent added/removed rows,
single-click clearing, scrolled Unicode selection at 17px, and selection-background
pixels. System clipboard checks can be skipped explicitly in environments where
Windows clipboard access is unavailable; source-text selection is still checked.
Word selection follows GPUI's editor boundaries: Latin word runs, whitespace
runs, and individual CJK characters or punctuation. Dragging can select any
continuous source range, preserving complete Unicode characters and tabs.

Implementation references (reviewed 2026-10-01):
[VS Code's original/modified editor ownership](https://github.com/microsoft/vscode/blob/dc546cc3c9979a19adafccd439889d7b64298def/src/vs/editor/browser/widget/diffEditor/components/diffEditorEditors.ts),
[Zed's layout-based position mapping](https://github.com/zed-industries/zed/blob/f8c2cc844057540ca1eac7de4f19f50d7597dead/crates/editor/src/element.rs#L10734),
and [Zed's character/word/line selection and focus](https://github.com/zed-industries/zed/blob/f8c2cc844057540ca1eac7de4f19f50d7597dead/crates/editor/src/selection.rs#L1220).
These inform interaction behavior; TShell remains a read-only diff viewer.
