use gpui_kit::{WindowAppearance, component::ThemeMode};
use serde::{Deserialize, Serialize};

// Fallback for UI palettes and bare rendering checks. App terminals replace
// these colours with the user's selected palette from terminal_theme.
pub(crate) const DEFAULT_ANSI: [u32; 16] = [
    0x172033, 0xf87171, 0x4ade80, 0xfbbf24, 0x60a5fa, 0xc084fc, 0x22d3ee, 0xdbe4f0, 0x64748b,
    0xfca5a5, 0x86efac, 0xfde68a, 0x93c5fd, 0xd8b4fe, 0x67e8f9, 0xffffff,
];

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Dark,
    Light,
}
impl Appearance {
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Dark,
            Self::Dark => Self::Light,
            Self::Light => Self::System,
        }
    }

    pub fn mode(self, system: WindowAppearance) -> ThemeMode {
        match self {
            Self::Dark => ThemeMode::Dark,
            Self::Light => ThemeMode::Light,
            Self::System => match system {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
                _ => ThemeMode::Light,
            },
        }
    }
}

fn mix(base: u32, tint: u32, amount: u8) -> u32 {
    let blend = |shift: u32| -> u32 {
        let a = (base >> shift) & 0xff_u32;
        let b = (tint >> shift) & 0xff_u32;
        (a * (255_u32 - u32::from(amount)) + b * u32::from(amount) + 127_u32) / 255_u32
    };
    blend(16) << 16 | blend(8) << 8 | blend(0)
}

fn contrast_text(background: u32) -> u32 {
    let luminance = [16, 8, 0]
        .into_iter()
        .zip([0.2126, 0.7152, 0.0722])
        .map(|(shift, weight)| {
            let channel = ((background >> shift) & 0xff) as f32 / 255.;
            let linear = if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            };
            linear * weight
        })
        .sum::<f32>();
    if luminance > 0.179 {
        0x000000
    } else {
        0xffffff
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub background: u32,
    pub panel: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub selected: u32,
    pub hover: u32,
    pub terminal: u32,
    pub selection: u32,
    pub error: u32,
    pub ansi: [u32; 16],
}
impl Palette {
    pub fn row_hover(self) -> u32 {
        self.hover
    }

    pub fn row_selected(self) -> u32 {
        self.selected
    }

    pub fn for_terminal(terminal: Self) -> Self {
        let mut palette = terminal;
        palette.background = terminal.terminal;
        palette.panel = mix(terminal.terminal, terminal.text, 18);
        palette.border = mix(terminal.terminal, terminal.text, 38);
        palette.muted = mix(terminal.text, terminal.terminal, 100);
        palette.hover = mix(palette.panel, palette.text, 20);
        palette.selected = if palette.terminal_theme().light() {
            mix(palette.panel, palette.text, 36)
        } else {
            mix(palette.panel, 0x000000, 30)
        };
        palette
    }

    pub fn with_interface(
        mut self,
        colors: Option<crate::terminal_theme::InterfaceColors>,
    ) -> Self {
        if let Some(colors) = colors {
            self.panel = colors.panel;
            self.border = colors.border;
            self.muted = colors.muted;
            self.hover = colors.hover;
            self.selected = colors.selected;
            self.error = colors.error;
        }
        self
    }

    pub fn apply_ui(self, theme: &mut gpui_kit::component::Theme) {
        use gpui_kit::rgb;

        let accent_foreground = rgb(contrast_text(self.accent)).into();
        let accent_hover = rgb(mix(self.accent, self.text, 25)).into();
        theme.background = rgb(self.background).into();
        theme.foreground = rgb(self.text).into();
        theme.accent = rgb(self.accent).into();
        theme.accent_foreground = accent_foreground;
        theme.accordion = rgb(self.panel).into();
        theme.group_box = rgb(self.panel).into();
        theme.group_box_foreground = rgb(self.text).into();
        theme.muted = rgb(self.panel).into();
        theme.muted_foreground = rgb(self.muted).into();
        theme.border = rgb(self.border).into();
        theme.input = rgb(self.border).into();
        theme.sidebar = rgb(self.panel).into();
        theme.sidebar_foreground = rgb(self.text).into();
        theme.sidebar_border = rgb(self.border).into();
        theme.sidebar_accent = rgb(self.row_selected()).into();
        theme.sidebar_accent_foreground = rgb(self.text).into();
        theme.sidebar_primary = rgb(self.accent).into();
        theme.sidebar_primary_foreground = accent_foreground;
        theme.title_bar = rgb(self.panel).into();
        theme.title_bar_border = rgb(self.border).into();
        theme.status_bar = rgb(self.panel).into();
        theme.status_bar_border = rgb(self.border).into();
        let light = self.terminal_theme().light();
        // Light input and select triggers use the theme background in GPUI Kit.
        theme.popover = rgb(if light { self.background } else { self.panel }).into();
        theme.popover_foreground = rgb(self.text).into();
        let overlay: gpui_kit::Hsla = rgb(if light { self.text } else { 0x000000 }).into();
        theme.overlay = overlay.alpha(if light { 0.26 } else { 0.55 });
        theme.button = rgb(self.panel).into();
        theme.button_foreground = rgb(self.text).into();
        theme.button_hover = rgb(self.row_hover()).into();
        theme.button_primary = rgb(self.accent).into();
        theme.button_primary_hover = accent_hover;
        theme.button_primary_active = accent_hover;
        theme.button_primary_foreground = accent_foreground;
        theme.secondary = rgb(self.panel).into();
        theme.secondary_foreground = rgb(self.text).into();
        theme.secondary_hover = rgb(self.row_hover()).into();
        theme.colors.list = rgb(self.panel).into();
        theme.list_hover = rgb(self.row_hover()).into();
        theme.list_active = rgb(self.row_selected()).into();
        theme.list_active_border = rgb(self.border).into();
        theme.list_head = rgb(self.panel).into();
        theme.tab_bar = rgb(self.panel).into();
        theme.tab_bar_segmented = rgb(self.panel).into();
        theme.tab = rgb(self.panel).into();
        theme.tab_active = rgb(self.row_selected()).into();
        theme.tab_active_foreground = rgb(self.text).into();
        theme.table = rgb(self.panel).into();
        theme.table_hover = rgb(self.row_hover()).into();
        theme.table_active = rgb(self.row_selected()).into();
        theme.table_head = rgb(self.panel).into();
        theme.table_foot = rgb(self.panel).into();
        theme.primary = rgb(self.accent).into();
        theme.primary_hover = accent_hover;
        theme.primary_active = accent_hover;
        theme.primary_foreground = accent_foreground;
        theme.link = rgb(self.accent).into();
        theme.link_hover = accent_hover;
        theme.link_active = accent_hover;
        theme.selection = rgb(self.selection).into();
        theme.scrollbar = rgb(self.panel).into();
        theme.scrollbar_thumb = rgb(self.border).into();
        theme.scrollbar_thumb_hover = rgb(self.muted).into();
        // The ring renderer replaces alpha, so a transparent color cannot hide it.
        theme.focus_ring = false;
        theme.ring = theme.input;
        theme.caret = rgb(self.accent).into();
        theme.tokens = (&theme.colors).into();
    }

    pub fn terminal_theme(self) -> crate::terminal_protocol::TerminalTheme {
        crate::terminal_protocol::TerminalTheme {
            foreground: self.text,
            background: self.terminal,
            cursor: self.accent,
            ansi: self.ansi,
        }
    }
    pub fn new(mode: ThemeMode) -> Self {
        if mode == ThemeMode::Light {
            Self {
                background: 0xffffff,
                panel: 0xffffff,
                border: 0xe5e5e5,
                text: 0x000000,
                muted: 0x666666,
                accent: 0x007acc,
                selected: 0xe5e5e5,
                hover: 0xececec,
                terminal: 0xffffff,
                selection: 0xadd6ff,
                error: 0xee0000,
                ansi: DEFAULT_ANSI,
            }
        } else {
            Self {
                background: 0x1e1e1e,
                panel: 0x1e1e1e,
                border: 0x3e3e42,
                text: 0xd4d4d4,
                muted: 0x858585,
                accent: 0x007acc,
                selected: 0x151515,
                hover: 0x2d2d2d,
                terminal: 0x1e1e1e,
                selection: 0x264f78,
                error: 0xf44747,
                ansi: DEFAULT_ANSI,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_follows_terminal_without_changing_its_color_report() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let mut terminal = Palette::new(mode);
            terminal.terminal = 0x242b31;
            terminal.text = 0xe8edf0;
            terminal.accent = 0x339cff;
            terminal.selection = 0x345678;
            let palette = Palette::for_terminal(terminal);
            assert_eq!(palette.background, terminal.terminal);
            assert_eq!(palette.text, terminal.text);
            assert_eq!(palette.accent, terminal.accent);
            assert_eq!(palette.panel, mix(terminal.terminal, terminal.text, 18));
            assert_eq!(palette.border, mix(terminal.terminal, terminal.text, 38));
            assert_eq!(palette.hover, mix(palette.panel, palette.text, 20));
            assert_eq!(palette.selected, palette.row_selected());
            assert_eq!(palette.terminal_theme(), terminal.terminal_theme());
            assert_eq!(palette.selection, terminal.selection);
        }
    }

    #[test]
    fn accent_text_remains_legible() {
        assert_eq!(contrast_text(0xffffff), 0x000000);
        assert_eq!(contrast_text(0x000000), 0xffffff);
        assert_eq!(contrast_text(0x8752a0), 0xffffff);
        assert_eq!(contrast_text(0xf4cd56), 0x000000);
    }

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
