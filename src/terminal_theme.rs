//! Terminal palettes are loaded from the user's single theme configuration file.
use crate::{appearance::Palette, terminal_protocol::TerminalTheme};
use gpui_kit::component::ThemeMode;

mod custom;
pub use custom::{ThemeDefinition, ThemeFile};

pub const DEFAULT_THEME_ID: &str = "one-dark";

pub fn paired_theme(id: &str, light: bool) -> Option<&'static str> {
    match id {
        "codex" | "codex-light" | "codex-dark" => {
            Some(if light { "codex-light" } else { "codex-dark" })
        }
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
        .expect("embedded theme template must contain one-dark")
        .palette()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_defaults_are_valid_and_complete() {
        let file = ThemeFile::default();
        assert_eq!(file.themes.len(), 21);
        assert_eq!(file.selected(DEFAULT_THEME_ID).unwrap().name, "One Dark");
        assert_eq!(file.themes.first().unwrap().id, "tide-light");
        for (id, foreground, background, cursor) in [
            ("codex-light", 0x1a1c1f, 0xffffff, 0x339cff),
            ("codex-dark", 0xffffff, 0x181818, 0x339cff),
            ("vscode-light", 0x3b3b3b, 0xffffff, 0x005fb8),
            ("vscode-dark", 0xcccccc, 0x1f1f1f, 0x0078d4),
        ] {
            let colors = file.selected(id).unwrap().colors();
            assert_eq!(
                (colors.foreground, colors.background, colors.cursor),
                (foreground, background, cursor)
            );
        }
        for (id, red, green, magenta) in [
            ("codex-light", 0xba2623, 0x00a240, 0x924ff7),
            ("codex-dark", 0xfa423e, 0x40c977, 0xad7bf9),
        ] {
            let ansi = file.selected(id).unwrap().colors().ansi;
            assert_eq!((ansi[1], ansi[2], ansi[5]), (red, green, magenta));
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
    fn embedded_defaults_match_the_source_snapshot() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/theme-catalog.json")).unwrap();
        let file = ThemeFile::default();
        let themes = fixture["themes"].as_array().unwrap();
        assert_eq!(themes.len(), 17);
        let ansi_keys = [
            "black",
            "red",
            "green",
            "yellow",
            "blue",
            "magenta",
            "cyan",
            "white",
            "brightBlack",
            "brightRed",
            "brightGreen",
            "brightYellow",
            "brightBlue",
            "brightMagenta",
            "brightCyan",
            "brightWhite",
        ];
        for source in themes {
            let theme = file.selected(source["id"].as_str().unwrap()).unwrap();
            let color = |key: &str| source[key].as_u64().unwrap() as u32;
            assert_eq!(theme.name, source["label"].as_str().unwrap());
            assert_eq!(theme.colors().foreground, color("foreground"));
            assert_eq!(theme.colors().background, color("background"));
            assert_eq!(theme.colors().cursor, color("cursor"));
            assert_eq!(theme.selection(), color("selectionBackground"));
            for (index, key) in ansi_keys.into_iter().enumerate() {
                assert_eq!(
                    theme.colors().ansi[index],
                    color(key),
                    "{}: {key}",
                    theme.id
                );
            }
        }
    }

    #[test]
    fn paired_themes_follow_lightness_only_for_new_families() {
        assert_eq!(paired_theme("codex", false), Some("codex-dark"));
        assert_eq!(paired_theme("codex-dark", true), Some("codex-light"));
        assert_eq!(paired_theme("vscode-light", false), Some("vscode-dark"));
        assert_eq!(paired_theme("one-dark", true), None);
    }
}
