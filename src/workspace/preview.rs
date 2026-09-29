use crate::appearance::Palette;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::text::TextView;
use gpui_kit::{prelude::*, *};
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom, SerializableHandle};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Block {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph(String),
    ListItem {
        ordered: bool,
        index: usize,
        text: String,
    },
    Code(String),
    Quote(String),
    Rule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Document {
    pub(super) blocks: Vec<Block>,
    content: Content,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Content {
    Markdown(String),
    Html(String),
}

impl Document {
    pub(super) fn markdown_source(&self) -> Option<&str> {
        match &self.content {
            Content::Markdown(source) => Some(source),
            Content::Html(_) => None,
        }
    }

    pub(super) fn web_html(&self) -> Option<&str> {
        match &self.content {
            Content::Markdown(_) => None,
            Content::Html(html) => Some(html),
        }
    }
}

pub(super) const MAX_PREVIEW_BYTES: usize = 256 * 1024;

pub(super) fn is_previewable(language: &str) -> bool {
    matches!(language, "markdown" | "html")
}

pub(super) fn can_preview(language: &str, source: &str) -> bool {
    is_previewable(language) && source.len() <= MAX_PREVIEW_BYTES
}

pub(super) fn parse(language: &str, source: &str) -> Document {
    if language == "markdown" {
        return Document {
            blocks: Vec::new(),
            content: Content::Markdown(source.to_owned()),
        };
    }
    Document {
        blocks: parse_markup(source),
        content: Content::Html(web_html(source)),
    }
}

pub(super) fn render_markdown(document: &Document, palette: Palette) -> Option<AnyElement> {
    let source = document.markdown_source()?;
    Some(
        div()
            .id("file-markdown-preview")
            .flex_1()
            .min_h_0()
            .child(
                TextView::markdown("file-markdown-text", source.to_owned())
                    .scrollable(true)
                    .selectable(true)
                    .on_link_click(|_, _, _, _| {})
                    .size_full()
                    .p_4()
                    .text_color(rgb(palette.text)),
            )
            .into_any_element(),
    )
}

fn web_html(markup: &str) -> String {
    const POLICY: &str = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'; frame-src 'none'; object-src 'none'";
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(markup);
    let policy = html5ever::parse_document(RcDom::default(), Default::default()).one(format!(
        "<meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{POLICY}\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">"
    ));
    if let (Some(head), Some(policy_head)) = (find_head(&dom.document), find_head(&policy.document))
    {
        let nodes = policy_head
            .children
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        for node in &nodes {
            node.parent.set(Some(std::rc::Rc::downgrade(&head)));
        }
        head.children.borrow_mut().splice(..0, nodes);
    }
    let mut output = Vec::new();
    if html5ever::serialize(
        &mut output,
        &SerializableHandle::from(dom.document),
        Default::default(),
    )
    .is_ok()
    {
        String::from_utf8(output).unwrap_or_default()
    } else {
        String::new()
    }
}

fn find_head(root: &Handle) -> Option<Handle> {
    let mut stack = vec![root.clone()];
    while let Some(node) = stack.pop() {
        if matches!(&node.data, NodeData::Element { name, .. } if name.local.as_ref() == "head") {
            return Some(node);
        }
        stack.extend(node.children.borrow().iter().rev().cloned());
    }
    None
}

#[cfg(windows)]
pub(super) struct WebPreview {
    webview: wry::WebView,
    html: String,
    bounds: Bounds<Pixels>,
    visible: bool,
}

#[cfg(windows)]
impl WebPreview {
    pub(super) fn new(html: &str, bounds: Bounds<Pixels>, window: &Window) -> wry::Result<Self> {
        let webview = wry::WebViewBuilder::new()
            .with_html(html)
            .with_incognito(true)
            .with_navigation_handler(|url| url == "about:blank")
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_bounds(web_bounds(bounds))
            .build_as_child(window)?;
        Ok(Self {
            webview,
            html: html.to_owned(),
            bounds,
            visible: true,
        })
    }

    pub(super) fn show(&mut self, html: &str, bounds: Bounds<Pixels>) -> wry::Result<()> {
        if self.html != html {
            self.webview.load_html(html)?;
            self.html = html.to_owned();
        }
        if self.bounds != bounds {
            self.webview.set_bounds(web_bounds(bounds))?;
            self.bounds = bounds;
        }
        if !self.visible {
            self.webview.set_visible(true)?;
            self.visible = true;
        }
        Ok(())
    }

    pub(super) fn hide(&mut self) {
        if self.visible && self.webview.set_visible(false).is_ok() {
            self.visible = false;
        }
    }

    pub(super) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(super) fn html(&self) -> &str {
        &self.html
    }
}

#[cfg(windows)]
fn web_bounds(bounds: Bounds<Pixels>) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(
            f64::from(bounds.origin.x),
            f64::from(bounds.origin.y),
        )
        .into(),
        size: wry::dpi::LogicalSize::new(
            f64::from(bounds.size.width),
            f64::from(bounds.size.height),
        )
        .into(),
    }
}

pub(super) fn render(document: &Document, palette: Palette, font_family: &str) -> AnyElement {
    let mut content = div()
        .id("file-preview-content")
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .text_color(rgb(palette.text));

    for block in &document.blocks {
        let element: AnyElement = match block {
            Block::Heading { level, text } => div()
                .text_size(px(match level {
                    1 => 26.,
                    2 => 22.,
                    3 => 19.,
                    4 => 17.,
                    5 => 15.,
                    _ => 14.,
                }))
                .line_height(relative(1.25))
                .font_weight(FontWeight::BOLD)
                .child(text.clone())
                .into_any_element(),
            Block::Paragraph(text) => div()
                .text_size(px(14.))
                .line_height(relative(1.55))
                .child(text.clone())
                .into_any_element(),
            Block::ListItem {
                ordered,
                index,
                text,
            } => div()
                .flex()
                .items_start()
                .gap_2()
                .text_size(px(14.))
                .line_height(relative(1.5))
                .child(
                    div()
                        .w(px(20.))
                        .flex_shrink_0()
                        .text_color(rgb(palette.accent))
                        .child(if *ordered {
                            format!("{index}.")
                        } else {
                            "•".to_owned()
                        }),
                )
                .child(div().min_w_0().flex_1().child(text.clone()))
                .into_any_element(),
            Block::Code(text) => div()
                .w_full()
                .p_3()
                .rounded_sm()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgba((palette.panel << 8) | 180))
                .font_family(font_family.to_owned())
                .text_size(px(12.))
                .line_height(relative(1.45))
                .whitespace_nowrap()
                .overflow_x_scrollbar()
                .child(text.clone())
                .into_any_element(),
            Block::Quote(text) => div()
                .w_full()
                .pl_3()
                .border_l_2()
                .border_color(rgb(palette.accent))
                .text_color(rgb(palette.muted))
                .text_size(px(14.))
                .line_height(relative(1.5))
                .child(text.clone())
                .into_any_element(),
            Block::Rule => div()
                .w_full()
                .h(px(1.))
                .bg(rgb(palette.border))
                .into_any_element(),
        };
        content = content.child(element);
    }

    div()
        .id("file-preview-scroll")
        .w_full()
        .flex_1()
        .min_h_0()
        .overflow_y_scrollbar()
        .child(content)
        .into_any_element()
}

#[derive(Clone, Copy)]
enum ActiveBlock {
    Paragraph,
    Heading(u8),
    Code,
    Quote,
    ListItem { ordered: bool, index: usize },
}

fn parse_markup(markup: &str) -> Vec<Block> {
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(markup);
    let mut blocks = Vec::new();
    let mut active = None;
    let mut text = String::new();
    let mut ordered_list = false;
    let mut list_index = 1;
    visit(
        dom.document,
        &mut blocks,
        &mut active,
        &mut text,
        &mut ordered_list,
        &mut list_index,
    );
    finish_block(&mut blocks, &mut active, &mut text);
    blocks
}

fn visit(
    root: Handle,
    blocks: &mut Vec<Block>,
    active: &mut Option<ActiveBlock>,
    text: &mut String,
    ordered_list: &mut bool,
    list_index: &mut usize,
) {
    let mut stack = vec![(root, false)];
    while let Some((node, closing)) = stack.pop() {
        let tag = match &node.data {
            NodeData::Element { name, .. } => name.local.as_ref(),
            NodeData::Text { contents } => {
                text.push_str(contents.borrow().as_ref());
                continue;
            }
            _ => "",
        };
        if matches!(
            tag,
            "head" | "script" | "style" | "iframe" | "object" | "embed" | "template"
        ) {
            continue;
        }
        process_tag(blocks, active, text, ordered_list, list_index, tag, closing);
        if !closing {
            stack.push((node.clone(), true));
            stack.extend(
                node.children
                    .borrow()
                    .iter()
                    .rev()
                    .cloned()
                    .map(|child| (child, false)),
            );
        }
    }
}

fn process_tag(
    blocks: &mut Vec<Block>,
    active: &mut Option<ActiveBlock>,
    text: &mut String,
    ordered_list: &mut bool,
    list_index: &mut usize,
    tag: &str,
    closing: bool,
) {
    if closing {
        match tag {
            "p" if matches!(
                active,
                Some(ActiveBlock::Quote | ActiveBlock::ListItem { .. })
            ) => {}
            "p" | "div" | "section" | "article" | "main" | "header" | "footer" | "pre"
            | "blockquote" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                finish_block(blocks, active, text);
            }
            "ul" | "ol" => {
                finish_block(blocks, active, text);
                *ordered_list = false;
                *list_index = 1;
            }
            _ => {}
        }
        return;
    }
    match tag {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            finish_block(blocks, active, text);
            *active = Some(ActiveBlock::Heading(tag.as_bytes()[1].saturating_sub(b'0')));
        }
        "p" | "div" | "section" | "article" | "main" | "header" | "footer" => {
            if tag == "p"
                && matches!(
                    active,
                    Some(ActiveBlock::Quote | ActiveBlock::ListItem { .. })
                )
            {
                return;
            }
            finish_block(blocks, active, text);
            *active = Some(ActiveBlock::Paragraph);
        }
        "pre" => {
            finish_block(blocks, active, text);
            *active = Some(ActiveBlock::Code);
        }
        "blockquote" => {
            finish_block(blocks, active, text);
            *active = Some(ActiveBlock::Quote);
        }
        "ul" => {
            finish_block(blocks, active, text);
            *ordered_list = false;
            *list_index = 1;
        }
        "ol" => {
            finish_block(blocks, active, text);
            *ordered_list = true;
            *list_index = 1;
        }
        "li" => {
            finish_block(blocks, active, text);
            *active = Some(ActiveBlock::ListItem {
                ordered: *ordered_list,
                index: *list_index,
            });
            *list_index += 1;
        }
        "br" => text.push('\n'),
        "hr" => {
            finish_block(blocks, active, text);
            blocks.push(Block::Rule);
        }
        "tr" => {
            finish_block(blocks, active, text);
            *active = Some(ActiveBlock::Paragraph);
        }
        "td" | "th" if !text.trim().is_empty() => text.push_str(" | "),
        _ => {}
    }
}

fn finish_block(blocks: &mut Vec<Block>, active: &mut Option<ActiveBlock>, text: &mut String) {
    let Some(kind) = active.take() else {
        let value = text.split_whitespace().collect::<Vec<_>>().join(" ");
        text.clear();
        if !value.is_empty() {
            blocks.push(Block::Paragraph(value));
        }
        return;
    };
    let value = if matches!(kind, ActiveBlock::Code) {
        text.trim_matches(['\n', '\r']).to_owned()
    } else {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    text.clear();
    if value.is_empty() {
        return;
    }
    blocks.push(match kind {
        ActiveBlock::Paragraph => Block::Paragraph(value),
        ActiveBlock::Heading(level) => Block::Heading { level, text: value },
        ActiveBlock::Code => Block::Code(value),
        ActiveBlock::Quote => Block::Quote(value),
        ActiveBlock::ListItem { ordered, index } => Block::ListItem {
            ordered,
            index,
            text: value,
        },
    });
}

#[cfg(test)]
mod tests {
    use super::{Block, MAX_PREVIEW_BYTES, can_preview, parse};

    #[test]
    fn preview_size_does_not_limit_source_editing() {
        assert!(can_preview("markdown", "# Hi"));
        assert!(!can_preview("markdown", &"a".repeat(MAX_PREVIEW_BYTES + 1)));
        assert!(!can_preview("plaintext", "short"));
    }

    #[test]
    fn markdown_preview_preserves_source_for_text_view() {
        let source =
            "# Title\n\nA **bold** paragraph.\n\n- one\n- two\n\n```rust\nfn main() {}\n```";
        let document = parse("markdown", source);
        assert_eq!(document.markdown_source(), Some(source));
        assert!(document.web_html().is_none());
    }

    #[test]
    fn html_preview_removes_active_content() {
        let document = parse(
            "html",
            "<h1>Hello</h1><script>alert('x')</script><p>World &amp; more</p>",
        );
        assert!(document.blocks.contains(&Block::Heading {
            level: 1,
            text: "Hello".into()
        }));
        assert!(
            document
                .blocks
                .contains(&Block::Paragraph("World & more".into()))
        );
        assert!(
            !document
                .blocks
                .iter()
                .any(|block| format!("{block:?}").contains("alert"))
        );
        assert_eq!(
            parse("html", "plain text").blocks,
            vec![Block::Paragraph("plain text".into())]
        );
    }

    #[test]
    fn web_preview_preserves_html_document_and_restricts_active_content() {
        let document = parse(
            "html",
            "<!doctype html><html lang=\"en\"><head><title>Page</title><style>p{color:red}</style></head><body class=\"custom\"><p>Content</p><script>alert(1)</script></body></html>",
        );
        let html = document.web_html().unwrap();
        assert!(html.contains("<html lang=\"en\">"));
        assert!(html.contains("<body class=\"custom\">"));
        assert!(html.contains("<style>p{color:red}</style>"));
        assert!(html.contains("script-src 'none'"));
        assert!(html.contains("default-src 'none'"));
        assert!(html.find("Content-Security-Policy") < html.find("<script>"));
    }
}
