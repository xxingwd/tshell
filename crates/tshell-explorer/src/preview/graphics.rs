use base64::{Engine, engine::general_purpose::STANDARD};

pub(super) fn page(language: &str, source: &str) -> String {
    let script = if language == "mermaid" {
        "<script defer src=\"/mermaid.js\"></script>"
    } else {
        ""
    };
    let title = escape(&crate::t!("file.graphic_preview"));
    let loading = escape(&crate::t!("file.preview_loading"));
    let failed = escape(&crate::t!("file.graphic_preview_failed"));
    let zoom_out = escape(&crate::t!("file.zoom_out"));
    let zoom_in = escape(&crate::t!("file.zoom_in"));
    let fit = escape(&crate::t!("file.zoom_fit"));
    let actual = escape(&crate::t!("file.zoom_actual"));
    let encoded = STANDARD.encode(source.as_bytes());
    format!(
        r#"<!doctype html>
<html lang="{locale}" data-renderer="{language}"><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'; frame-src 'none'; object-src 'none'; connect-src 'none'">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>{title}</title>
<style>
*{{box-sizing:border-box}}body{{margin:0;font:13px system-ui;color:#1f2328;background:white;display:flex;flex-direction:column;height:100vh}}
nav{{height:32px;flex-shrink:0;display:flex;gap:4px;align-items:center;padding:2px 8px;border-bottom:1px solid #d0d7de;background:#f6f8fa}}
button{{min-width:28px;height:26px;padding:0 6px;border:1px solid transparent;border-radius:3px;background:transparent;color:inherit;cursor:pointer;font:inherit}}
button:hover{{background:#e6e9ed}}button:focus-visible{{outline:2px solid #0969da;outline-offset:-2px}}button:disabled{{opacity:.45;cursor:default}}output{{min-width:46px;text-align:center}}
main{{flex:1;min-height:0;overflow:auto;padding:16px}}img{{display:block;margin:auto}}#status{{margin:16px;white-space:pre-wrap;overflow-wrap:anywhere}}[hidden]{{display:none!important}}
</style>{script}<script defer src="/viewer.js"></script></head><body>
<nav aria-label="{title}"><button id="out" title="{zoom_out}" aria-label="{zoom_out}" disabled>−</button><output id="scale">100%</output><button id="in" title="{zoom_in}" aria-label="{zoom_in}" disabled>+</button><button id="fit" title="{fit}" disabled>{fit}</button><button id="actual" title="{actual}" disabled>1:1</button></nav>
<p id="status" role="status" aria-live="polite" data-error="{failed}">{loading}</p>
<main id="viewport" tabindex="0" aria-label="{title}" aria-busy="true"><img id="graphic" alt="{title}" hidden></main>
<div id="source" data-source="{encoded}" hidden></div>
</body></html>"#,
        locale = escape(&crate::i18n::current_locale())
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphic_source_cannot_inject_markup_or_script_into_viewer() {
        let source = "</div><script>alert('x')</script><svg onload=\"evil()\"/>";
        let html = page("svg", source);
        assert!(!html.contains(source));
        assert!(html.contains(&STANDARD.encode(source)));
        assert!(html.contains("script-src 'self'"));
        assert!(html.contains("connect-src 'none'"));
        assert!(!html.contains("/mermaid.js"));
        assert!(page("mermaid", "graph TD; A-->B").contains("/mermaid.js"));
    }
}
