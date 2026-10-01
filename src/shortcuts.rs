//! Defaults follow the workspace reference shortcut layout.
use crate::backend::Direction;
use gpui_kit::Keystroke;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    CommandPalette,
    Copy,
    TerminalSearch,
    Paste,
    ShowTerminal,
    ShowFiles,
    ShowGit,
    NewWindow,
    CloseWindow,
    AddHost,
    EditHost,
    RemoveHost,
    NextHost,
    PreviousHost,
    RefreshFiles,
    RefreshGit,
    Reconnect,
    SaveFile,
    Settings,
    CycleTheme,
    ClosePane,
    ToggleSidebar,
    Zen,
    RenderStats,
    Focus,
    Floating,
    Layout,
    Font(i32),
    Size(i32),
    NewColumn,
    NewRow,
    Move(Direction),
    MovePane(Direction),
    Session(usize),
}
pub type Overrides = std::collections::BTreeMap<String, Vec<String>>;
pub struct Binding {
    pub id: &'static str,
    /// Translation key; resolve it through [`Binding::label`].
    key: &'static str,
    pub action: Shortcut,
    pub defaults: &'static [&'static str],
}
impl Binding {
    pub fn label(&self) -> String {
        crate::t!(self.key).to_string()
    }
}
pub const BINDINGS: &[Binding] = &[
    Binding {
        id: "terminal_search",
        key: "shortcut.terminal_search",
        action: Shortcut::TerminalSearch,
        defaults: &["ctrl-shift-f"],
    },
    Binding {
        id: "command_palette",
        key: "shortcut.command_palette",
        action: Shortcut::CommandPalette,
        defaults: &["ctrl-shift-k"],
    },
    Binding {
        id: "show_terminal",
        key: "shortcut.show_terminal",
        action: Shortcut::ShowTerminal,
        defaults: &["ctrl-1"],
    },
    Binding {
        id: "show_files",
        key: "shortcut.show_files",
        action: Shortcut::ShowFiles,
        defaults: &["ctrl-2"],
    },
    Binding {
        id: "show_git",
        key: "shortcut.show_git",
        action: Shortcut::ShowGit,
        defaults: &["ctrl-3"],
    },
    Binding {
        id: "new_window",
        key: "shortcut.new_window",
        action: Shortcut::NewWindow,
        defaults: &["ctrl-shift-t"],
    },
    Binding {
        id: "close_window",
        key: "shortcut.close_window",
        action: Shortcut::CloseWindow,
        defaults: &["ctrl-shift-w"],
    },
    Binding {
        id: "add_host",
        key: "shortcut.add_host",
        action: Shortcut::AddHost,
        defaults: &[],
    },
    Binding {
        id: "edit_host",
        key: "shortcut.edit_host",
        action: Shortcut::EditHost,
        defaults: &[],
    },
    Binding {
        id: "remove_host",
        key: "shortcut.remove_host",
        action: Shortcut::RemoveHost,
        defaults: &[],
    },
    Binding {
        id: "previous_host",
        key: "shortcut.previous_host",
        action: Shortcut::PreviousHost,
        defaults: &["ctrl-alt-left"],
    },
    Binding {
        id: "next_host",
        key: "shortcut.next_host",
        action: Shortcut::NextHost,
        defaults: &["ctrl-alt-right"],
    },
    Binding {
        id: "refresh_files",
        key: "shortcut.refresh_files",
        action: Shortcut::RefreshFiles,
        defaults: &[],
    },
    Binding {
        id: "refresh_git",
        key: "shortcut.refresh_git",
        action: Shortcut::RefreshGit,
        defaults: &[],
    },
    Binding {
        id: "reconnect",
        key: "shortcut.reconnect",
        action: Shortcut::Reconnect,
        defaults: &[],
    },
    Binding {
        id: "save_file",
        key: "shortcut.save_file",
        action: Shortcut::SaveFile,
        defaults: &[],
    },
    Binding {
        id: "settings",
        key: "shortcut.settings",
        action: Shortcut::Settings,
        defaults: &[],
    },
    Binding {
        id: "cycle_theme",
        key: "shortcut.cycle_theme",
        action: Shortcut::CycleTheme,
        defaults: &[],
    },
    Binding {
        id: "close_pane",
        key: "shortcut.close_pane",
        action: Shortcut::ClosePane,
        defaults: &[],
    },
    Binding {
        id: "toggle_sidebar",
        key: "shortcut.toggle_sidebar",
        action: Shortcut::ToggleSidebar,
        defaults: &["ctrl-b"],
    },
    Binding {
        id: "zen",
        key: "shortcut.zen",
        action: Shortcut::Zen,
        defaults: &["ctrl-shift-z"],
    },
    Binding {
        id: "render_stats",
        key: "shortcut.render_stats",
        action: Shortcut::RenderStats,
        defaults: &["ctrl-shift-p"],
    },
    Binding {
        id: "focus",
        key: "shortcut.focus",
        action: Shortcut::Focus,
        defaults: &["alt-enter"],
    },
    Binding {
        id: "floating",
        key: "shortcut.floating",
        action: Shortcut::Floating,
        defaults: &["alt-f"],
    },
    Binding {
        id: "layout",
        key: "shortcut.layout",
        action: Shortcut::Layout,
        defaults: &["alt-t"],
    },
    Binding {
        id: "font_down",
        key: "shortcut.font_down",
        action: Shortcut::Font(-1),
        defaults: &["ctrl--"],
    },
    Binding {
        id: "font_up",
        key: "shortcut.font_up",
        action: Shortcut::Font(1),
        defaults: &["ctrl-="],
    },
    Binding {
        id: "size_down",
        key: "shortcut.size_down",
        action: Shortcut::Size(-1),
        defaults: &["alt--"],
    },
    Binding {
        id: "size_up",
        key: "shortcut.size_up",
        action: Shortcut::Size(1),
        defaults: &["alt-="],
    },
    Binding {
        id: "column",
        key: "shortcut.column",
        action: Shortcut::NewColumn,
        defaults: &["alt-n"],
    },
    Binding {
        id: "row",
        key: "shortcut.row",
        action: Shortcut::NewRow,
        defaults: &["alt-shift-n"],
    },
    Binding {
        id: "move_left",
        key: "shortcut.move_left",
        action: Shortcut::Move(Direction::Left),
        defaults: &["alt-h", "alt-left"],
    },
    Binding {
        id: "move_down",
        key: "shortcut.move_down",
        action: Shortcut::Move(Direction::Down),
        defaults: &["alt-j", "alt-down"],
    },
    Binding {
        id: "move_up",
        key: "shortcut.move_up",
        action: Shortcut::Move(Direction::Up),
        defaults: &["alt-k", "alt-up"],
    },
    Binding {
        id: "move_right",
        key: "shortcut.move_right",
        action: Shortcut::Move(Direction::Right),
        defaults: &["alt-l", "alt-right"],
    },
    Binding {
        id: "pane_left",
        key: "shortcut.pane_left",
        action: Shortcut::MovePane(Direction::Left),
        defaults: &["alt-shift-h", "alt-shift-left"],
    },
    Binding {
        id: "pane_down",
        key: "shortcut.pane_down",
        action: Shortcut::MovePane(Direction::Down),
        defaults: &["alt-shift-j", "alt-shift-down"],
    },
    Binding {
        id: "pane_up",
        key: "shortcut.pane_up",
        action: Shortcut::MovePane(Direction::Up),
        defaults: &["alt-shift-k", "alt-shift-up"],
    },
    Binding {
        id: "pane_right",
        key: "shortcut.pane_right",
        action: Shortcut::MovePane(Direction::Right),
        defaults: &["alt-shift-l", "alt-shift-right"],
    },
    Binding {
        id: "session_1",
        key: "shortcut.session",
        action: Shortcut::Session(0),
        defaults: &["ctrl-alt-1"],
    },
    Binding {
        id: "session_2",
        key: "shortcut.session",
        action: Shortcut::Session(1),
        defaults: &["ctrl-alt-2"],
    },
    Binding {
        id: "session_3",
        key: "shortcut.session",
        action: Shortcut::Session(2),
        defaults: &["ctrl-alt-3"],
    },
    Binding {
        id: "session_4",
        key: "shortcut.session",
        action: Shortcut::Session(3),
        defaults: &["ctrl-alt-4"],
    },
    Binding {
        id: "session_5",
        key: "shortcut.session",
        action: Shortcut::Session(4),
        defaults: &["ctrl-alt-5"],
    },
    Binding {
        id: "session_6",
        key: "shortcut.session",
        action: Shortcut::Session(5),
        defaults: &["ctrl-alt-6"],
    },
    Binding {
        id: "session_7",
        key: "shortcut.session",
        action: Shortcut::Session(6),
        defaults: &["ctrl-alt-7"],
    },
    Binding {
        id: "session_8",
        key: "shortcut.session",
        action: Shortcut::Session(7),
        defaults: &["ctrl-alt-8"],
    },
    Binding {
        id: "session_9",
        key: "shortcut.session",
        action: Shortcut::Session(8),
        defaults: &["ctrl-alt-9"],
    },
    Binding {
        id: "copy",
        key: "shortcut.copy",
        action: Shortcut::Copy,
        defaults: &["ctrl-shift-c"],
    },
    Binding {
        id: "paste",
        key: "shortcut.paste",
        action: Shortcut::Paste,
        defaults: &["ctrl-shift-v"],
    },
];
pub fn keys(binding: &Binding, overrides: &Overrides) -> Vec<String> {
    overrides
        .get(binding.id)
        .cloned()
        .unwrap_or_else(|| binding.defaults.iter().map(|s| s.to_string()).collect())
}
fn matches(key: &Keystroke, text: &str) -> bool {
    Keystroke::parse(text).is_ok_and(|other| {
        other.modifiers == key.modifiers && other.key.eq_ignore_ascii_case(&key.key)
    })
}
pub struct Keymap(Vec<(Keystroke, Shortcut)>);
impl Keymap {
    pub fn new(overrides: &Overrides) -> Self {
        Self(
            BINDINGS
                .iter()
                .flat_map(|b| {
                    keys(b, overrides)
                        .into_iter()
                        .filter_map(|text| Keystroke::parse(&text).ok().map(|key| (key, b.action)))
                })
                .collect(),
        )
    }
    pub fn resolve(&self, key: &Keystroke) -> Option<Shortcut> {
        self.0
            .iter()
            .find(|(other, _)| {
                other.modifiers == key.modifiers && other.key.eq_ignore_ascii_case(&key.key)
            })
            .map(|(_, action)| *action)
    }
}
#[cfg(test)]
pub fn resolve_with(key: &Keystroke, overrides: &Overrides) -> Option<Shortcut> {
    Keymap::new(overrides).resolve(key)
}
#[cfg(test)]
pub fn resolve(key: &Keystroke) -> Option<Shortcut> {
    resolve_with(key, &Overrides::new())
}
pub fn assign(index: usize, key: &Keystroke, overrides: &mut Overrides) -> Result<(), String> {
    let m = key.modifiers;
    let function = key
        .key
        .strip_prefix('f')
        .and_then(|s| s.parse::<u8>().ok())
        .is_some_and(|i| (1..=24).contains(&i));
    if m.platform || (!m.control && !m.alt && !function) {
        return Err(crate::t!("shortcut.error_modifier").to_string());
    }
    if (m.alt && key.key == "f4") || (m.control && m.alt && key.key == "delete") {
        return Err(crate::t!("shortcut.error_reserved").to_string());
    }
    if let Some(conflict) = BINDINGS
        .iter()
        .enumerate()
        .find(|(i, b)| *i != index && keys(b, overrides).iter().any(|text| matches(key, text)))
    {
        return Err(crate::t!("shortcut.error_conflict", label = conflict.1.label()).to_string());
    }
    overrides.insert(BINDINGS[index].id.into(), vec![key.unparse()]);
    Ok(())
}
pub fn display(text: &str) -> String {
    Keystroke::parse(text)
        .map(|key| {
            let mut parts = Vec::new();
            if key.modifiers.control {
                parts.push("Ctrl".to_string());
            }
            if key.modifiers.alt {
                parts.push("Alt".to_string());
            }
            if key.modifiers.shift {
                parts.push("Shift".to_string());
            }
            parts.push(key.key.to_uppercase());
            parts.join("+")
        })
        .unwrap_or_else(|_| text.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overrides_replace_aliases_reject_conflicts_and_round_trip() {
        let index = BINDINGS.iter().position(|b| b.id == "move_left").unwrap();
        let mut config = Overrides::new();
        assert!(assign(index, &Keystroke::parse("alt-n").unwrap(), &mut config).is_err());
        assert!(config.is_empty());
        assert!(assign(index, &Keystroke::parse("a").unwrap(), &mut config).is_err());
        assert!(assign(index, &Keystroke::parse("alt-f4").unwrap(), &mut config).is_err());
        assign(
            index,
            &Keystroke::parse("ctrl-shift-h").unwrap(),
            &mut config,
        )
        .unwrap();
        let saved = serde_json::to_string(&config).unwrap();
        let restored: Overrides = serde_json::from_str(&saved).unwrap();
        for old in ["alt-h", "alt-left"] {
            assert_eq!(
                resolve_with(&Keystroke::parse(old).unwrap(), &restored),
                None
            );
        }
        assert_eq!(
            resolve_with(&Keystroke::parse("ctrl-shift-h").unwrap(), &restored),
            Some(Shortcut::Move(Direction::Left))
        );
        assert_eq!(
            resolve_with(&Keystroke::parse("ctrl-c").unwrap(), &restored),
            None
        );
        assert_eq!(
            resolve_with(&Keystroke::parse("ctrl-shift-c").unwrap(), &restored),
            Some(Shortcut::Copy)
        );
    }
    #[test]
    fn workspace_defaults_and_terminal_passthrough() {
        for (key, action) in [
            ("alt-n", Shortcut::NewColumn),
            ("alt-shift-n", Shortcut::NewRow),
            ("alt-enter", Shortcut::Focus),
            ("ctrl-1", Shortcut::ShowTerminal),
            ("ctrl-2", Shortcut::ShowFiles),
            ("ctrl-3", Shortcut::ShowGit),
            ("ctrl-shift-t", Shortcut::NewWindow),
            ("ctrl-shift-w", Shortcut::CloseWindow),
            ("alt-h", Shortcut::Move(Direction::Left)),
            ("alt-shift-left", Shortcut::MovePane(Direction::Left)),
            ("ctrl-=", Shortcut::Font(1)),
            ("ctrl-alt-2", Shortcut::Session(1)),
            ("ctrl-b", Shortcut::ToggleSidebar),
            ("ctrl-shift-z", Shortcut::Zen),
            ("ctrl-shift-p", Shortcut::RenderStats),
            ("ctrl-shift-k", Shortcut::CommandPalette),
        ] {
            assert_eq!(
                resolve(&Keystroke::parse(key).unwrap()),
                Some(action),
                "{key}"
            );
        }
        for key in ["ctrl-c", "ctrl-d", "ctrl-z", "alt-b", "a"] {
            assert_eq!(resolve(&Keystroke::parse(key).unwrap()), None, "{key}");
        }
    }
}
