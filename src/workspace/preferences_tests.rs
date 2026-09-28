use super::{Appearance, Preferences, active_theme_id, theme_slots};
use crate::i18n::Language;
use crate::terminal_theme::ThemeFile;
use gpui_kit::WindowAppearance;

#[test]
fn system_mode_selects_independently_configured_light_and_dark_schemes() {
    let themes = ThemeFile::default();
    for (appearance, system, expected) in [
        (Appearance::Light, WindowAppearance::Dark, "github-light"),
        (Appearance::Dark, WindowAppearance::Light, "vscode-dark"),
        (Appearance::System, WindowAppearance::Light, "github-light"),
        (Appearance::System, WindowAppearance::Dark, "vscode-dark"),
    ] {
        assert_eq!(
            active_theme_id(&themes, "github-light", "vscode-dark", appearance, system),
            expected
        );
    }
    assert_eq!(
        active_theme_id(
            &themes,
            "missing",
            "vscode-dark",
            Appearance::Light,
            WindowAppearance::Dark
        ),
        themes.fallback_id()
    );
}

#[test]
fn legacy_theme_choice_migrates_to_two_editable_slots() {
    let themes = ThemeFile::default();
    for (legacy, expected_light, expected_dark) in [
        ("codex", "codex-light", "codex-dark"),
        ("vscode-light", "vscode-light", "vscode-dark"),
        ("one-dark", "tide-light", "one-dark"),
    ] {
        let prefs: Preferences =
            serde_json::from_str(&format!(r#"{{"selected_theme":"{legacy}"}}"#)).unwrap();
        assert_eq!(
            theme_slots(&themes, &prefs),
            (expected_light.into(), expected_dark.into())
        );
    }
    let prefs: Preferences = serde_json::from_str(
        r#"{"selected_theme":"codex","light_theme":"github-light","dark_theme":"vscode-dark"}"#,
    )
    .unwrap();
    assert_eq!(
        theme_slots(&themes, &prefs),
        ("github-light".into(), "vscode-dark".into())
    );
    let restored: Preferences =
        serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
    assert_eq!(
        theme_slots(&themes, &restored),
        theme_slots(&themes, &prefs)
    );
    let mut missing = themes.clone();
    missing.themes.retain(|theme| theme.id != "vscode-dark");
    assert_eq!(
        theme_slots(&missing, &prefs),
        ("github-light".into(), "codex-dark".into())
    );
    assert_eq!(
        theme_slots(&themes, &Preferences::default()),
        ("tide-light".into(), "one-dark".into())
    );
}

#[test]
fn language_preference_defaults_to_system_and_round_trips() {
    let bare: Preferences = serde_json::from_str("{}").unwrap();
    assert_eq!(bare.window_size, None);
    assert_eq!(bare.show_top_title, None);
    assert_eq!(bare.show_status_bar, None);
    assert_eq!(bare.line_height_scale.factor(), 1.2);
    assert_eq!(bare.language, Language::System);
    assert_eq!(bare.selected_theme, None);
    for (json, expected) in [
        (r#"{"language":"system"}"#, Language::System),
        (r#"{"language":"zh-cn"}"#, Language::ZhCn),
        (r#"{"language":"en"}"#, Language::En),
    ] {
        let prefs: Preferences = serde_json::from_str(json).unwrap();
        assert_eq!(prefs.language, expected);
        let encoded = serde_json::to_string(&prefs).unwrap();
        let restored: Preferences = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.language, expected);
    }
}

#[test]
fn chrome_visibility_preferences_round_trip() {
    let prefs: Preferences =
        serde_json::from_str(r#"{"show_top_title":false,"show_status_bar":false}"#).unwrap();
    assert_eq!(prefs.show_top_title, Some(false));
    assert_eq!(prefs.show_status_bar, Some(false));
    let restored: Preferences =
        serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
    assert_eq!(restored.show_top_title, Some(false));
    assert_eq!(restored.show_status_bar, Some(false));
}

#[test]
fn sidebar_opacity_defaults_to_opaque_and_round_trips_independently() {
    let mut prefs: Preferences = serde_json::from_str(r#"{"background_opacity":0.7}"#).unwrap();
    assert_eq!(prefs.resolved_sidebar_opacity(), 1.);
    prefs.sidebar_opacity = Some(0.8);
    assert_eq!(prefs.resolved_sidebar_opacity(), 0.8);
    let restored: Preferences =
        serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
    assert_eq!(restored.resolved_sidebar_opacity(), 0.8);
    assert_eq!(restored.background_opacity, Some(0.7));
}

#[test]
fn preferences_replace_existing_file_and_restore_window_size() {
    let directory = std::env::temp_dir().join(format!(
        "tshell-preferences-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path = directory.join("workspace.json");
    let mut prefs = Preferences::default();
    prefs.window_size = Some([1200., 700.]);
    prefs.write_to(&path).unwrap();
    prefs.window_size = Some([1420., 900.]);
    prefs.write_to(&path).unwrap();
    let restored: Preferences = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(restored.window_size, prefs.window_size);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn old_preferences_and_unknown_theme_keep_unrelated_settings() {
    // The previous terminal_appearance field is ignored, while hosts and other
    // preferences survive the move to independent, named terminal themes.
    let saved = r#"{
        "terminal_appearance":"light", "appearance":"light",
        "font_family":"Consolas", "font_size":16, "background_opacity":0.65,
        "local_name":"My terminal", "sidebar_collapsed":true
    }"#;
    for input in [
        saved.to_string(),
        saved.replace(
            "\"terminal_appearance\":\"light\"",
            "\"terminal_theme\":\"future-theme\"",
        ),
    ] {
        let prefs: Preferences = serde_json::from_str::<Preferences>(&input)
            .unwrap()
            .migrated();
        let expected = input
            .contains("terminal_theme")
            .then(|| "future-theme".to_string());
        assert_eq!(prefs.selected_theme, expected);
        assert_eq!(prefs.font_family, "Consolas");
        assert_eq!(prefs.font_size, 16.);
        assert_eq!(prefs.background_opacity, Some(0.65));
        assert_eq!(prefs.local_name, "My terminal");
        assert!(prefs.sidebar_collapsed);
        assert_eq!(
            prefs.metrics_config,
            super::metrics_config::Config::default()
        );
        assert!(prefs.appearance == Appearance::Light);
    }
    let mut prefs: Preferences = serde_json::from_str(saved).unwrap();
    prefs.selected_theme = Some("solarized-light".into());
    let encoded = serde_json::to_string(&prefs).unwrap();
    let restored: Preferences = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored.selected_theme, Some("solarized-light".into()));
    assert_eq!(restored.background_opacity, prefs.background_opacity);
}

#[test]
fn custom_theme_selection_round_trips_without_changing_builtin_theme() {
    let prefs: Preferences = serde_json::from_str(
        r#"{"terminal_theme":"nord-dark","selected_custom_theme":"custom-2"}"#,
    )
    .unwrap();
    let migrated = prefs.migrated();
    assert_eq!(migrated.selected_theme.as_deref(), Some("custom-2"));
    let restored: Preferences =
        serde_json::from_str(&serde_json::to_string(&migrated).unwrap()).unwrap();
    assert_eq!(restored.selected_theme.as_deref(), Some("custom-2"));
}

#[test]
fn terminal_grid_centers_columns_and_leaves_extra_height_below() {
    let viewport = crate::backend::PixelViewport {
        width: 990.,
        height: 590.,
        cell_width: 10.,
        line_height: 20.,
    };
    let (cols, rows) = viewport.terminal_size();
    let padding = crate::backend::PANE_PADDING;
    let left = super::grid_inset(viewport.width, cols, viewport.cell_width, padding);
    let right = viewport.width - left - cols as f32 * viewport.cell_width;
    let top = super::grid_inset(viewport.height, rows, viewport.line_height, padding);
    let bottom = viewport.height - top - rows as f32 * viewport.line_height;
    assert_eq!((cols, rows), (98, 29));
    assert_eq!((left, right), (8., 2.));
    assert_eq!((top, bottom), (8., 2.));

    // A naturally larger inset stays centered instead of gaining another 8px.
    assert_eq!(super::grid_inset(1000., 98, 10., padding), 10.);
}

#[test]
fn pane_dividers_join_across_local_and_tmux_gaps() {
    for gap in [1., 8., 21.] {
        let total = 200. + gap;
        let (upper_offset, upper_length) = super::divider_span(0., 100., total, gap);
        let (lower_offset, lower_length) = super::divider_span(100. + gap, 100., total, gap);
        let upper_end = upper_offset + upper_length;
        let lower_start = 100. + gap + lower_offset;
        assert_eq!(upper_offset, 0.);
        assert_eq!(lower_start + lower_length, total);
        assert_eq!(upper_end - lower_start, super::PANE_STROKE);
        // Either adjacent pane's frame covers the exact same divider stroke.
        let divider_start = 100. + (gap - super::PANE_STROKE) / 2.;
        assert_eq!(upper_end - super::PANE_STROKE, divider_start);
        assert_eq!(lower_start, divider_start);
        assert_eq!(super::divider_span(0., total, total, gap), (0., total));
    }
}

#[test]
fn session_storage_excludes_tmux_and_ignores_display_names() {
    let mut host = crate::tmux_client::HostConfig {
        destination: "host".into(),
        name: "first".into(),
        user: String::new(),
        port: None,
        identity_file: None,
        tmux: false,
        socket: None,
    };
    let key = super::session_storage_key(Some(&host));
    host.name = "renamed".into();
    assert_eq!(super::session_storage_key(Some(&host)), key);
    host.user = "other".into();
    assert_ne!(super::session_storage_key(Some(&host)), key);
    host.tmux = true;
    assert_eq!(super::session_storage_key(Some(&host)), None);
    assert_eq!(super::session_storage_key(None).as_deref(), Some("local"));
}

#[test]
fn host_credentials_round_trip_without_password_field() {
    let host: crate::tmux_client::HostConfig = serde_json::from_str(
        r#"{"destination":"example.test","user":"deploy","port":2222,"identity_file":"C:/keys/work","tmux":false}"#,
    )
    .unwrap();
    assert_eq!(host.user, "deploy");
    assert_eq!(
        host.identity_file.as_deref(),
        Some(std::path::Path::new("C:/keys/work"))
    );
    let saved = serde_json::to_value(host).unwrap();
    assert_eq!(saved["user"], "deploy");
    assert!(saved.get("password").is_none());
}

#[test]
fn active_pane_highlights_only_existing_dividers() {
    use gpui_kit::px;
    for gap in [1., 8., 21.] {
        let pane = super::PaneBounds {
            id: "pane".into(),
            x: 0.,
            y: 0.,
            width: 100.,
            height: 100.,
        };
        let total = 200. + gap;
        let lines = super::active_dividers(&pane, total, total, gap, gap);
        assert_eq!(lines.len(), 2);
        let edge = 100. + (gap - super::PANE_STROKE) / 2.;
        assert_eq!(lines[0].origin.x, px(edge));
        assert_eq!(lines[0].size.width, px(super::PANE_STROKE));
        assert_eq!(lines[1].origin.y, px(edge));
        assert_eq!(lines[1].size.height, px(super::PANE_STROKE));
        assert!(super::active_dividers(&pane, 100., 100., gap, gap).is_empty());
        let opposite = super::PaneBounds {
            x: 100. + gap,
            y: 100. + gap,
            ..pane
        };
        let lines = super::active_dividers(&opposite, total, total, gap, gap);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].origin.x, px(edge));
        assert_eq!(lines[1].origin.y, px(edge));
    }
}
