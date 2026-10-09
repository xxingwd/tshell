#[cfg(test)]
mod tests {
    use crate::terminal_theme::ThemeFile;
    use gpui_kit::rgb;
    use tshell_ui::editor::style;
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
