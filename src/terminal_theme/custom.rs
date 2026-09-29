use super::{DEFAULT_THEME_ID, InterfaceColors, Palette, TerminalTheme, palette};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{collections::HashSet, path::Path};

const DEFAULT_THEME_JSON: &str = include_str!("defaults.json");
const THEME_FILE_VERSION: u32 = 3;
const REMOVED_BUILTIN_THEME_IDS: &[&str] = &[
    "tide-light",
    "tide-dark",
    "github-light",
    "github-dark",
    "nord-light",
    "nord-dark",
    "solarized-light",
    "solarized-dark",
    "gruvbox-light",
    "gruvbox-dark",
    "catppuccin-latte",
    "catppuccin-mocha",
    "tokyo-day",
    "tokyo-night",
    "dracula",
    "monokai",
    "codex-light",
    "codex-dark",
    "one-dark",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HexColor(u32);

impl Serialize for HexColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("#{:06X}", self.0))
    }
}

impl<'de> Deserialize<'de> for HexColor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.len() != 7 || !text.starts_with('#') {
            return Err(de::Error::custom("expected #RRGGBB"));
        }
        if !text.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit) {
            return Err(de::Error::custom("expected #RRGGBB"));
        }
        u32::from_str_radix(&text[1..], 16)
            .map(Self)
            .map_err(|_| de::Error::custom("expected #RRGGBB"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InterfaceTheme {
    panel: HexColor,
    border: HexColor,
    muted: HexColor,
    hover: HexColor,
    selected: HexColor,
    error: HexColor,
}

impl InterfaceTheme {
    fn colors(&self) -> InterfaceColors {
        InterfaceColors {
            panel: self.panel.0,
            border: self.border.0,
            muted: self.muted.0,
            hover: self.hover.0,
            selected: self.selected.0,
            error: self.error.0,
        }
    }

    fn defaults(theme: TerminalTheme) -> Self {
        let palette = Palette::for_terminal(palette(theme, 0));
        Self {
            panel: HexColor(palette.panel),
            border: HexColor(palette.border),
            muted: HexColor(palette.muted),
            hover: HexColor(palette.hover),
            selected: HexColor(palette.selected),
            error: HexColor(palette.error),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeDefinition {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(rename = "ink", alias = "foreground")]
    foreground: HexColor,
    #[serde(rename = "surface", alias = "background")]
    background: HexColor,
    #[serde(rename = "accent", alias = "cursor")]
    cursor: HexColor,
    selection: HexColor,
    ansi: [HexColor; 16],
    #[serde(default)]
    interface: Option<InterfaceTheme>,
}

impl ThemeDefinition {
    pub fn from_theme(theme: &Self, id: String) -> Self {
        let mut copy = theme.clone();
        copy.id = id;
        copy.name = format!("{} Custom", theme.name);
        copy
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 64
                && self
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
            "invalid theme id: {}",
            self.id
        );
        ensure!(
            !self.name.trim().is_empty() && self.name.chars().count() <= 80,
            "theme name must contain 1 to 80 characters"
        );
        Ok(())
    }

    pub fn colors(&self) -> TerminalTheme {
        TerminalTheme {
            foreground: self.foreground.0,
            background: self.background.0,
            cursor: self.cursor.0,
            ansi: self.ansi.map(|color| color.0),
        }
    }

    pub fn selection(&self) -> u32 {
        self.selection.0
    }

    pub fn palette(&self) -> Palette {
        palette(self.colors(), self.selection())
    }

    pub fn ui_palette(&self) -> Palette {
        Palette::for_terminal(self.palette()).with_interface(self.interface_colors())
    }

    fn interface_colors(&self) -> Option<InterfaceColors> {
        self.interface.as_ref().map(InterfaceTheme::colors)
    }

    pub(crate) fn ensure_interface(&mut self) {
        if self.interface.is_none() {
            self.interface = Some(InterfaceTheme::defaults(self.colors()));
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeFile {
    #[serde(default)]
    pub version: u32,
    pub themes: Vec<ThemeDefinition>,
}

impl Default for ThemeFile {
    fn default() -> Self {
        let file: Self = serde_json::from_str(DEFAULT_THEME_JSON)
            .expect("embedded theme template must be valid");
        file.validate()
            .expect("embedded theme template must be valid");
        file
    }
}

impl ThemeFile {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let file: Self = serde_json::from_slice(bytes)?;
        file.validate()?;
        Ok(file)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.version <= THEME_FILE_VERSION,
            "unsupported theme file version: {}",
            self.version
        );
        if self.version != 0 {
            ensure!(
                !self.themes.is_empty(),
                "theme file must contain at least one theme"
            );
        }
        let mut ids = HashSet::new();
        for theme in &self.themes {
            theme.validate()?;
            ensure!(ids.insert(&theme.id), "duplicate theme id: {}", theme.id);
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut file =
            Self::parse(&std::fs::read(path).with_context(|| path.display().to_string())?)?;
        let defaults = Self::default();
        let mut changed = false;
        for theme in &mut file.themes {
            if theme.interface.is_none() {
                theme.interface = defaults
                    .selected(&theme.id)
                    .and_then(|builtin| builtin.interface.clone());
                theme.ensure_interface();
                changed = true;
            }
        }
        let original_len = file.themes.len();
        file.themes
            .retain(|theme| !REMOVED_BUILTIN_THEME_IDS.contains(&theme.id.as_str()));
        changed |= file.themes.len() != original_len;
        for theme in defaults.themes {
            if file.selected(&theme.id).is_none() {
                file.themes.push(theme);
                changed = true;
            }
        }
        if file.version < THEME_FILE_VERSION {
            file.version = THEME_FILE_VERSION;
            changed = true;
        }
        if changed {
            file.write(path)?;
        }
        Ok(file)
    }

    pub fn ensure_file(path: &Path, legacy_path: &Path) -> Result<Self> {
        if path.exists() {
            return Self::load(path);
        }
        let mut file = Self::default();
        if legacy_path.exists() {
            let mut theme: ThemeDefinition = serde_json::from_slice(&std::fs::read(legacy_path)?)?;
            theme.id = file.next_id();
            theme.ensure_interface();
            file.themes.push(theme);
        }
        file.validate()?;
        std::fs::create_dir_all(path.parent().context("theme path has no parent")?)?;
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut output) => output.write_all(&serde_json::to_vec_pretty(&file)?)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Self::load(path);
            }
            Err(error) => return Err(error.into()),
        }
        Ok(file)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let mut file = self.clone();
        for theme in &mut file.themes {
            theme.ensure_interface();
        }
        file.version = THEME_FILE_VERSION;
        file.validate()?;
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(&file)?)?;
        if let Err(error) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(temporary);
            return Err(error.into());
        }
        Ok(())
    }

    pub fn next_id(&self) -> String {
        (1..)
            .map(|number| format!("custom-{number}"))
            .find(|id| self.themes.iter().all(|theme| theme.id != *id))
            .unwrap()
    }

    pub fn selected(&self, id: &str) -> Option<&ThemeDefinition> {
        self.themes.iter().find(|theme| theme.id == id)
    }

    pub fn fallback_id(&self) -> &str {
        self.selected(DEFAULT_THEME_ID)
            .or_else(|| self.themes.first())
            .map(|theme| theme.id.as_str())
            .expect("validated theme file must contain a theme")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_round_trips_and_rejects_bad_colors() {
        let theme = ThemeFile::default()
            .selected(DEFAULT_THEME_ID)
            .unwrap()
            .clone();
        let json = serde_json::to_string(&theme).unwrap();
        let restored: ThemeDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, theme);
        assert!(serde_json::from_str::<ThemeDefinition>(&json.replace('#', "0x")).is_err());
        assert!(serde_json::from_str::<ThemeDefinition>(&json.replace("#", "#FFFFFFF")).is_err());
        assert!(serde_json::from_str::<ThemeDefinition>(&json.replace("#", "#+")).is_err());
    }

    #[test]
    fn interface_colors_do_not_change_terminal_colors_or_selection() {
        let mut theme = ThemeFile::default()
            .selected("vscode-dark")
            .unwrap()
            .clone();
        let terminal = theme.palette();
        let interface = theme.interface.as_mut().unwrap();
        interface.panel = HexColor(0x262626);
        interface.hover = HexColor(0x333333);
        interface.selected = HexColor(0x171717);
        interface.error = HexColor(0xff0000);

        assert!(theme.palette() == terminal);
        let ui = theme.ui_palette();
        assert_eq!(
            (ui.panel, ui.hover, ui.selected, ui.error),
            (0x262626, 0x333333, 0x171717, 0xff0000)
        );
        assert_eq!(ui.terminal_theme(), terminal.terminal_theme());
        assert_eq!(ui.selection, terminal.selection);
    }

    #[test]
    fn version_two_themes_gain_interface_colors_without_losing_custom_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("theme.json");
        let mut built_in =
            serde_json::to_value(ThemeFile::default().selected("vscode-light").unwrap()).unwrap();
        let mut custom = built_in.clone();
        custom["id"] = "custom-1".into();
        custom["ink"] = "#123456".into();
        for theme in [&mut built_in, &mut custom] {
            let fields = theme.as_object_mut().unwrap();
            fields.remove("interface");
            for (new, old) in [
                ("ink", "foreground"),
                ("surface", "background"),
                ("accent", "cursor"),
            ] {
                let value = fields.remove(new).unwrap();
                fields.insert(old.into(), value);
            }
        }
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"version": 2, "themes": [built_in, custom]}))
                .unwrap(),
        )
        .unwrap();

        let migrated = ThemeFile::load(&path).unwrap();
        assert_eq!(migrated.version, 3);
        assert_eq!(migrated.themes.len(), 3);
        let light = migrated.selected("vscode-light").unwrap();
        assert_eq!(light.ui_palette().border, 0xe5e5e5);
        let custom = migrated.selected("custom-1").unwrap();
        assert_eq!(custom.colors().foreground, 0x123456);
        assert_eq!(
            custom.ui_palette().panel,
            Palette::for_terminal(custom.palette()).panel
        );
        assert_eq!(ThemeFile::load(&path).unwrap(), migrated);
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(saved["themes"][0].get("foreground").is_none());
        assert_eq!(saved["themes"][0]["ink"], "#000000");
        assert!(saved["themes"][0]["interface"].is_object());
    }

    #[test]
    fn file_creation_and_migration_are_atomic() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("theme.json");
        let legacy = directory.path().join("terminal-theme.json");
        let theme = ThemeFile::default()
            .selected(DEFAULT_THEME_ID)
            .unwrap()
            .clone();
        std::fs::write(&legacy, serde_json::to_vec(&theme).unwrap()).unwrap();
        let migrated = ThemeFile::ensure_file(&path, &legacy).unwrap();
        assert_eq!(migrated.themes.len(), 3);
        assert_eq!(migrated.themes.last().unwrap().id, "custom-1");
        assert_eq!(ThemeFile::load(&path).unwrap(), migrated);
        let mut changed = migrated.clone();
        changed.themes.pop();
        changed.write(&path).unwrap();
        assert_eq!(ThemeFile::load(&path).unwrap(), changed);
    }

    #[test]
    fn old_theme_file_gets_default_themes_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("theme.json");
        let mut theme = ThemeFile::default()
            .selected(DEFAULT_THEME_ID)
            .unwrap()
            .clone();
        theme.foreground = HexColor(0x123456);
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"themes": [theme]})).unwrap(),
        )
        .unwrap();
        let migrated = ThemeFile::load(&path).unwrap();
        assert_eq!(migrated.version, 3);
        assert_eq!(migrated.themes.len(), 2);
        assert_eq!(
            migrated
                .selected(DEFAULT_THEME_ID)
                .unwrap()
                .colors()
                .foreground,
            0x123456
        );
        assert_eq!(ThemeFile::load(&path).unwrap(), migrated);
    }

    #[test]
    fn old_builtin_themes_are_removed_without_overwriting_vscode_edits() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("theme.json");
        let mut old = ThemeFile {
            version: 1,
            themes: vec![
                ThemeDefinition::from_theme(
                    ThemeFile::default().selected("vscode-dark").unwrap(),
                    "one-dark".into(),
                ),
                ThemeFile::default()
                    .selected("vscode-dark")
                    .unwrap()
                    .clone(),
            ],
        };
        old.version = 1;
        old.themes
            .iter_mut()
            .find(|theme| theme.id == "vscode-dark")
            .unwrap()
            .foreground = HexColor(0x123456);
        std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();

        let upgraded = ThemeFile::load(&path).unwrap();
        assert_eq!(upgraded.version, 3);
        assert_eq!(upgraded.themes.len(), 2);
        assert_eq!(
            upgraded
                .selected(DEFAULT_THEME_ID)
                .unwrap()
                .colors()
                .foreground,
            0x123456
        );
        assert!(upgraded.selected("one-dark").is_none());
        assert!(upgraded.selected("vscode-light").is_some());
        assert!(upgraded.selected("vscode-dark").is_some());
        assert_eq!(ThemeFile::load(&path).unwrap(), upgraded);
    }

    #[test]
    fn duplicate_and_empty_theme_files_are_rejected() {
        let mut file = ThemeFile::default();
        file.themes.push(file.themes[0].clone());
        assert!(ThemeFile::parse(&serde_json::to_vec(&file).unwrap()).is_err());
        file.themes.clear();
        assert!(ThemeFile::parse(&serde_json::to_vec(&file).unwrap()).is_err());
    }

    #[test]
    fn new_theme_is_derived_from_the_active_theme() {
        let file = ThemeFile::default();
        let theme = file.selected(DEFAULT_THEME_ID).unwrap();
        let custom = ThemeDefinition::from_theme(theme, "custom-1".into());
        assert_eq!(custom.id, "custom-1");
        assert_eq!(custom.name, "VS Code Dark Modern Custom");
        assert_eq!(custom.colors(), theme.colors());
    }
}
