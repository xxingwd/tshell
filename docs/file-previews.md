# Explorer file previews

Explorer reads local and SFTP files through the same preview model. Previewing
never writes the source. The main pane owns the preview; switching to the source
editor, another tool, a dialog, or another host hides its native child WebView.

| Format | Preview | Limit | Editing |
| --- | --- | --- | --- |
| Markdown (`.md`, `.mdx`) | Native rich text | 256 KiB | Edit/Preview |
| HTML (`.html`, `.htm`) | Windows WebView2; native text fallback elsewhere | 256 KiB | Edit/Preview |
| SVG (`.svg`) | Windows WebView2 image viewer | 256 KiB | Edit/Preview |
| Mermaid (`.mmd`, `.mermaid`) | Offline Mermaid 12.2.0 in Windows WebView2 | 32 KiB | Edit/Preview |
| PDF (`.pdf`) | Windows WebView2 PDF viewer | 20 MiB | Read-only |
| PNG (`.png`) | Native image | 20 MiB | Read-only |

Text files above the preview limit remain editable up to the editor's 20 MiB
limit. Oversized binary files are refused, never opened as an empty text editor.
Files with invalid PNG/PDF signatures are refused. A PDF remains read-only after
renaming, including a rename that changes its extension, until another file is
opened.

SVG and Mermaid previews provide zoom in/out, fit and actual-size controls. With
the image area focused, `+`, `-` and `0` zoom in, zoom out and fit. The document
canvas uses a white background to preserve SVG colours and Mermaid's default
theme independently of the surrounding workspace theme. Mermaid runs only for
standalone diagram files; Markdown fenced blocks continue using the native
Markdown renderer. SVG/Mermaid without WebView2 show a notice and the source;
Edit always remains available. Invalid syntax shows an error with an Edit
recovery instruction. Mermaid accepts at most 500 edges.

HTML retains its script-disabled content policy. SVG is decoded as an image,
which prevents embedded scripts, navigation, and external subresources from
running. Mermaid source is base64-encoded as data, passed to the bundled
renderer with `securityLevel: strict`, and its SVG output is also decoded as an
image. Its page permits only the packaged viewer and Mermaid scripts and blocks
network connections, frames, objects, forms and external resources. Relative
assets are not read from disk or SFTP. Links cannot navigate away or open new
windows. Browser downloads are disabled; SFTP downloads remain Explorer actions.

The in-memory protocols serve only the current document and the explicitly
bundled renderer assets. They start no HTTP server and expose no directory.
The [asset build instructions](../assets/preview/README.md) pin dependencies,
record hashes, and explain license packaging. Normal Cargo builds need no
JavaScript tooling.

Graphviz/PlantUML, EPUB, and Office conversion are deferred. JSON, YAML and code
continue using the native source editor.

## Verification

Run `cargo test --workspace --locked` for parsing, limits, binary rename state,
locale coverage, protocol resources and navigation boundaries. On Windows with
WebView2 installed, build a debug binary and run:

```powershell
$env:TSHELL_DATA_DIR = "$PWD/tmp/common-previews-check"
$env:TSHELL_UI_CHECK_SCOPE = 'common-previews'
$env:TSHELL_UI_CHECK_VISIBLE = '1'
./target/debug/tshell.exe --workspace-ui-check "$PWD/tmp/common-previews-report.json"
```

Use an isolated directory and a separate debug binary to preserve running
terminal sessions. The check waits for the actual image decoder and Mermaid
renderer to report success or failure through WebView IPC; a visible child
window alone does not count as a rendered graphic. It exercises HTML, SVG,
Mermaid flowcharts/sequences, malformed diagrams, source switches and hiding the
preview. The small checked-in `tests/fixtures/explorer-preview.pdf` verifies PDF
navigation and that Save never writes PDF bytes through the text editor. This
check does not establish SFTP connectivity or full Mermaid syntax parity.
