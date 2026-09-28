//! UI localisation.
//!
//! Translation text lives in `locales/*.yml` and is loaded at compile time by
//! `rust-i18n`. Only user-facing interface text is translated: terminal bytes,
//! command output, remote file contents and names the user typed stay verbatim.
//!
//! The locale is process-global because `gpui-component` shares the same
//! `rust_i18n` backend, so setting it here also localises the component
//! library's own text such as input context menus and select placeholders.
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Locale used when the system preference cannot be mapped to a supported one.
pub const FALLBACK_LOCALE: &str = "zh-CN";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    /// Follow the operating system, falling back to [`FALLBACK_LOCALE`].
    #[default]
    System,
    ZhCn,
    En,
}
impl Language {
    pub const ALL: &'static [Self] = &[Self::System, Self::ZhCn, Self::En];

    /// Locale tag this preference resolves to.
    pub fn locale(self) -> &'static str {
        match self {
            Self::System => system_locale(),
            Self::ZhCn => "zh-CN",
            Self::En => "en",
        }
    }

    pub fn label(self) -> Cow<'static, str> {
        t!(self.key())
    }

    fn key(self) -> &'static str {
        match self {
            Self::System => "settings.language_system",
            Self::ZhCn => "settings.language_zh_cn",
            Self::En => "settings.language_en",
        }
    }
}

/// Map the operating system locale onto one of the bundled locales.
///
/// Chinese variants follow Simplified Chinese. English follows English, and any
/// other language falls back to [`FALLBACK_LOCALE`] rather than showing text the
/// user is less likely to read than Chinese.
fn system_locale() -> &'static str {
    match sys_locale::get_locale() {
        Some(tag) if tag.starts_with("en") => "en",
        Some(tag) if tag.starts_with("zh") => "zh-CN",
        _ => FALLBACK_LOCALE,
    }
}

/// Apply a language preference to the process-wide locale.
pub fn set_language(language: Language) {
    rust_i18n::set_locale(language.locale());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    /// Serialise tests that mutate the process-global locale.
    static LOCALE: Mutex<()> = Mutex::new(());

    /// Collect nested leaf keys and their raw values from a locale YAML file
    /// without a YAML dependency.
    fn leaf_entries(path: &str) -> BTreeMap<String, String> {
        let text = std::fs::read_to_string(path).expect("locale file readable");
        let mut path_stack: Vec<(usize, String)> = Vec::new();
        let mut entries = BTreeMap::new();
        for line in text.lines() {
            if line.trim().is_empty() || line.starts_with('_') {
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            let trimmed = line.trim();
            while path_stack.last().is_some_and(|(i, _)| *i >= indent) {
                path_stack.pop();
            }
            if let Some(rest) = trimmed.strip_prefix('"') {
                if let Some((key, value)) = rest.split_once("\": ") {
                    let full = if let Some((_, parent)) = path_stack.last() {
                        format!("{parent}.{key}")
                    } else {
                        key.to_string()
                    };
                    let value = value
                        .strip_prefix('"')
                        .and_then(|v| v.strip_suffix('"'))
                        .unwrap_or(value);
                    entries.insert(full, value.to_string());
                }
            } else if let Some((key, value)) = trimmed.split_once(':') {
                if value.trim().is_empty() {
                    let key = key.trim().trim_matches('"');
                    let full = if let Some((_, parent)) = path_stack.last() {
                        format!("{parent}.{key}")
                    } else {
                        key.to_string()
                    };
                    path_stack.push((indent, full));
                }
            }
        }
        entries
    }

    fn leaf_keys(path: &str) -> BTreeSet<String> {
        leaf_entries(path).into_keys().collect()
    }

    /// `%{name}` placeholder names used by a translated message.
    fn placeholders(value: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let mut rest = value;
        while let Some(start) = rest.find("%{") {
            rest = &rest[start + 2..];
            match rest.find('}') {
                Some(end) => {
                    names.insert(rest[..end].to_string());
                    rest = &rest[end + 1..];
                }
                None => break,
            }
        }
        names
    }

    #[test]
    fn en_and_zh_cn_ship_the_same_keys() {
        let en = leaf_keys(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/en.yml"));
        let zh = leaf_keys(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/zh-CN.yml"));
        assert!(en.len() > 100, "en.yml parsed only {} keys", en.len());
        let only_en: Vec<_> = en.difference(&zh).collect();
        let only_zh: Vec<_> = zh.difference(&en).collect();
        assert!(
            only_en.is_empty() && only_zh.is_empty(),
            "locale key drift: only en {only_en:?}, only zh {only_zh:?}"
        );
    }

    #[test]
    fn placeholders_match_across_locales() {
        let en = leaf_entries(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/en.yml"));
        let zh = leaf_entries(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/zh-CN.yml"));
        for (key, en_value) in &en {
            let Some(zh_value) = zh.get(key) else {
                continue;
            };
            let en_names = placeholders(en_value);
            let zh_names = placeholders(zh_value);
            assert!(
                en_names == zh_names,
                "placeholder drift in {key}: en {en_names:?}, zh {zh_names:?}"
            );
        }
    }

    #[test]
    fn language_locales_map_without_touching_global_state() {
        assert!(!Language::System.locale().is_empty());
        assert_eq!(Language::ZhCn.locale(), "zh-CN");
        assert_eq!(Language::En.locale(), "en");
        assert_eq!(Language::En.key(), "settings.language_en");
        assert_eq!(Language::ZhCn.key(), "settings.language_zh_cn");
        assert_eq!(Language::System.key(), "settings.language_system");
    }

    #[test]
    fn titles_translate_per_locale() {
        let _guard = LOCALE.lock().unwrap_or_else(|e| e.into_inner());
        set_language(Language::En);
        assert_eq!(t!("settings.title"), "Settings");
        assert_eq!(Language::En.label(), "English");
        set_language(Language::ZhCn);
        assert_eq!(t!("settings.title"), "系统设置");
        assert_eq!(Language::ZhCn.label(), "简体中文");
        set_language(Language::System);
    }
}
