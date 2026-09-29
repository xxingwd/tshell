//! Terminal palettes are loaded from the user's single theme configuration file.
use crate::{appearance::Palette, terminal_protocol::TerminalTheme};
use gpui_kit::component::ThemeMode;

mod custom;
pub use custom::{ThemeDefinition, ThemeFile};

pub const DEFAULT_THEME_ID: &str = "vscode-dark";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InterfaceColors {
    pub panel: u32,
    pub border: u32,
    pub muted: u32,
    pub hover: u32,
    pub selected: u32,
    pub error: u32,
}

pub fn paired_theme(id: &str, light: bool) -> Option<&'static str> {
    match id {
        "vscode" | "vscode-light" | "vscode-dark" => {
            Some(if light { "vscode-light" } else { "vscode-dark" })
        }
        _ => None,
    }
}

pub(crate) fn palette(colors: TerminalTheme, selection: u32) -> Palette {
    let mut palette = Palette::new(if colors.light() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    });
    palette.background = colors.background;
    palette.terminal = colors.background;
    palette.text = colors.foreground;
    palette.accent = colors.cursor;
    palette.selected = selection;
    palette.selection = selection;
    palette.ansi = colors.ansi;
    palette
}

pub fn default_palette() -> Palette {
    ThemeFile::default()
        .selected(DEFAULT_THEME_ID)
        .expect("embedded theme template must contain vscode-dark")
        .palette()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_defaults_are_valid_and_complete() {
        let file = ThemeFile::default();
        assert_eq!(file.themes.len(), 2);
        assert_eq!(
            file.selected(DEFAULT_THEME_ID).unwrap().name,
            "VS Code Dark Modern"
        );
        assert_eq!(file.themes.first().unwrap().id, "vscode-light");
        for (id, foreground, background, cursor) in [
            ("vscode-light", 0x000000, 0xffffff, 0x007acc),
            ("vscode-dark", 0xd4d4d4, 0x1e1e1e, 0x007acc),
        ] {
            let colors = file.selected(id).unwrap().colors();
            assert_eq!(
                (colors.foreground, colors.background, colors.cursor),
                (foreground, background, cursor)
            );
        }
    }

    #[test]
    fn palettes_match_the_protocol_shape() {
        let file = ThemeFile::default();
        for theme in &file.themes {
            let palette = theme.palette();
            assert_eq!(palette.terminal_theme(), theme.colors());
            assert_eq!(palette.selection, theme.selection());
        }
    }

    #[test]
    fn paired_themes_follow_lightness_for_vscode() {
        assert_eq!(paired_theme("vscode-light", false), Some("vscode-dark"));
        assert_eq!(paired_theme("vscode-dark", true), Some("vscode-light"));
        assert_eq!(paired_theme("one-dark", true), None);
    }
}
