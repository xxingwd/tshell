#[cfg(test)]
use gpui_kit::component::ThemeMode;
pub use tshell_ui::appearance::{Appearance, Palette};
pub(crate) trait PaletteTerminalTheme {
    fn terminal_theme(self) -> crate::terminal_protocol::TerminalTheme;
}
impl PaletteTerminalTheme for Palette {
    fn terminal_theme(self) -> crate::terminal_protocol::TerminalTheme {
        crate::terminal_protocol::TerminalTheme {
            foreground: self.text,
            background: self.terminal,
            cursor: self.accent,
            selection: self.selection,
            ansi: self.ansi,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_interface_palettes_keep_their_semantic_colors() {
        let themes = crate::terminal_theme::ThemeFile::default();
        for (id, panel, error) in [
            ("vscode-light", 0xffffff, 0xee0000),
            ("vscode-dark", 0x1e1e1e, 0xf44747),
        ] {
            let ui = themes.selected(id).unwrap().ui_palette();
            assert_eq!(ui.panel, panel);
            assert_eq!(ui.error, error);
            assert_eq!(ui.background, ui.terminal);
            assert_eq!(ui.panel, ui.background);
            assert_eq!(ui.terminal_theme(), themes.selected(id).unwrap().colors());
        }
    }

    #[test]
    fn row_hover_contrasts_with_the_panel_in_both_modes() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let palette = Palette::new(mode);
            assert_ne!(palette.row_hover(), palette.panel);
            if mode == ThemeMode::Light {
                assert!(palette.row_hover() < palette.panel);
            } else {
                assert!(palette.row_hover() > palette.panel);
            }
        }
    }

    #[test]
    fn row_selected_is_deeper_than_the_panel_in_both_modes() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let palette = Palette::new(mode);
            assert!(palette.row_selected() < palette.panel);
            assert!(palette.row_selected() < palette.row_hover());
        }
    }

    #[test]
    fn focused_controls_keep_regular_border_without_outer_ring() {
        let schemes = crate::terminal_theme::ThemeFile::default();
        let mut theme = gpui_kit::component::Theme::default();
        for palette in [ThemeMode::Light, ThemeMode::Dark]
            .into_iter()
            .map(Palette::new)
            .chain(schemes.themes.iter().map(|scheme| scheme.ui_palette()))
        {
            theme.focus_ring = true;
            palette.apply_ui(&mut theme);
            assert!(!theme.focus_ring);
            assert_eq!(theme.ring, theme.input);
            assert_eq!(theme.input, gpui_kit::rgb(palette.border).into());
        }
    }

    #[test]
    fn popover_matches_control_surface_in_light_themes() {
        let mut theme = gpui_kit::component::Theme::default();
        let schemes = crate::terminal_theme::ThemeFile::default();
        for scheme in &schemes.themes {
            let id = scheme.id.as_str();
            let palette = scheme.ui_palette();
            let light = palette.terminal_theme().light();
            theme.mode = if light {
                ThemeMode::Light
            } else {
                ThemeMode::Dark
            };
            palette.apply_ui(&mut theme);
            assert_eq!(
                theme.tokens.background.background,
                gpui_kit::rgb(palette.background).into()
            );
            assert_eq!(
                theme.tokens.popover.background,
                gpui_kit::rgb(if light {
                    palette.background
                } else {
                    palette.panel
                })
                .into(),
                "{id}"
            );
            if light {
                assert_eq!(theme.popover, theme.input_background(), "{id}");
            } else {
                assert_eq!(
                    theme.tokens.popover.background,
                    theme.tokens.sidebar.background
                );
            }
            let overlay: gpui_kit::Hsla =
                gpui_kit::rgb(if light { palette.text } else { 0x000000 }).into();
            assert_eq!(
                theme.overlay,
                overlay.alpha(if light { 0.26 } else { 0.55 })
            );
            assert_eq!(
                theme.tokens.border.background,
                gpui_kit::rgb(palette.border).into()
            );
            assert_eq!(
                theme.list_active_border,
                gpui_kit::rgb(palette.border).into()
            );
            assert_eq!(palette.terminal_theme(), scheme.colors());
        }
    }
}
