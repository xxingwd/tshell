use crate::appearance::Palette;
use gpui_kit::{
    base::input::{
        DiagnosticColors, EditorState, FoldRange, HighlightStyleResolver, InputEdit,
        InputEditorStyle, InputHighlighter, InputHighlighterFactory, Rope,
    },
    component::highlighter::SyntaxHighlighter,
    *,
};
use std::{cell::RefCell, ops::Range, rc::Rc, sync::Arc, time::Duration};

struct Colors(Palette);
impl HighlightStyleResolver for Colors {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        let palette = self.0;
        let color = match name.split('.').next()? {
            "comment" => palette.ansi[8],
            "keyword" | "operator" => palette.ansi[5],
            "string" => palette.ansi[2],
            "number" | "constant" | "boolean" => palette.ansi[3],
            "function" | "constructor" => palette.ansi[4],
            "type" | "tag" | "attribute" => palette.ansi[6],
            _ => palette.text,
        };
        Some(HighlightStyle {
            color: Some(rgb(color).into()),
            ..Default::default()
        })
    }
}

pub(super) fn style(p: Palette) -> InputEditorStyle {
    InputEditorStyle {
        foreground: rgb(p.text).into(),
        muted_foreground: rgb(p.ansi[8]).into(),
        background: rgb(p.terminal).into(),
        border: rgb(p.ansi[8]).into(),
        selection: rgb(p.selection).into(),
        caret: rgb(p.accent).into(),
        diagnostics: DiagnosticColors {
            error: rgb(p.ansi[1]).into(),
            warning: rgb(p.ansi[3]).into(),
            info: rgb(p.ansi[4]).into(),
            hint: rgb(p.ansi[6]).into(),
        },
        highlight_styles: Arc::new(Colors(p)),
        editor_gutter_background: Some(rgb(p.terminal).into()),
        editor_active_line: Some(rgba((p.text << 8) | 10).into()),
        ..Default::default()
    }
}

// Keep parsing off the UI thread. Replacing the task drops stale parse results.
struct Highlighter {
    language: SharedString,
    parsed: Rc<RefCell<Option<SyntaxHighlighter>>>,
    task: Option<Task<()>>,
}

pub(super) fn factory() -> InputHighlighterFactory {
    Rc::new(|language| {
        Some(Box::new(Highlighter {
            language: language.to_owned().into(),
            parsed: Rc::default(),
            task: None,
        }))
    })
}

impl InputHighlighter for Highlighter {
    fn language(&self) -> SharedString {
        self.language.clone()
    }
    fn update(
        &mut self,
        _: Option<InputEdit>,
        text: &Rope,
        _: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        self.parsed.borrow_mut().take();
        let text = text.clone();
        let language = self.language.clone();
        let parsed = self.parsed.clone();
        self.task = Some(cx.spawn_in(window, async move |state, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(80))
                .await;
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut highlighter = SyntaxHighlighter::new(&language);
                    highlighter.update(None, &text, Some(Duration::from_millis(200)));
                    highlighter
                })
                .await;
            parsed.replace(Some(result));
            let _ = state.update(cx, |_, cx| cx.notify());
        }));
    }
    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        self.parsed
            .borrow()
            .as_ref()
            .map(|parsed| parsed.styles(range, resolver))
            .unwrap_or_else(|| vec![(range.clone(), HighlightStyle::default())])
    }
    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::style;
    use crate::terminal_theme::ThemeFile;
    use gpui_kit::rgb;
    #[test]
    fn editor_colors_follow_every_terminal_theme() {
        for theme in &ThemeFile::default().themes {
            let p = theme.palette();
            let style = style(p);
            assert_eq!(style.background, rgb(p.terminal).into());
            assert_eq!(style.foreground, rgb(p.text).into());
            assert_eq!(style.caret, rgb(p.accent).into());
            assert_eq!(style.selection, rgb(p.selection).into());
            assert_eq!(
                style.highlight_styles.style("string").unwrap().color,
                Some(rgb(p.ansi[2]).into())
            );
        }
    }
}
