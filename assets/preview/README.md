# Offline preview assets

`viewer.js` is TShell's SVG/Mermaid viewer. The checked-in `mermaid.js` is a
single browser bundle of Mermaid **12.2.0**, built with esbuild **0.28.2**.
Rust embeds these assets in the executable and serves them only through the
Explorer preview protocol. Node.js and pnpm are packaging tools, not application
runtime dependencies. No CDN, renderer service, Graphviz, EPUB or Office engine
is used.

Rebuild from the repository root with Node.js 24 and pnpm 11.25.0:

```powershell
pnpm --dir tools/preview-renderers install --frozen-lockfile --ignore-scripts
pnpm --dir tools/preview-renderers build
```

The build prints SHA-256 hashes and collects license texts for every bundled
package in `THIRD_PARTY_LICENSES.txt`. Windows packaging appends those notices
to the distributed notices file. `mermaid.js.LEGAL.txt` preserves upstream
license comments.

Current `mermaid.js` SHA-256:
`0a1130ca0675d49906c568123172d3ada7551daa2413318105986018de4cff72`.

Primary upstream documentation: [Mermaid usage](https://mermaid.js.org/config/usage.html),
[Mermaid source](https://github.com/mermaid-js/mermaid).
