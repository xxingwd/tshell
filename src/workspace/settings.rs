use super::*;
use gpui_kit::component::{
    searchable_list::SearchableListItem,
    select::{SearchableVec, Select, SelectEvent, SelectState},
    setting::{SelectIndex, SettingGroup, SettingItem, SettingPage, Settings},
    slider::{Slider, SliderEvent, SliderState},
};

#[derive(Clone)]
struct SchemeItem {
    id: String,
    name: String,
}

impl SearchableListItem for SchemeItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone().into()
    }

    fn value(&self) -> &String {
        &self.id
    }
}

fn scheme_items(themes: &ThemeFile) -> SearchableVec<SchemeItem> {
    SearchableVec::new(
        themes
            .themes
            .iter()
            .map(|theme| SchemeItem {
                id: theme.id.clone(),
                name: theme.name.clone(),
            })
            .collect::<Vec<_>>(),
    )
}

#[derive(Clone)]
struct MetricDrag {
    metric: metrics_config::Metric,
    background: u32,
    foreground: u32,
}
impl Render for MetricDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .rounded_md()
            .shadow_md()
            .bg(rgb(self.background))
            .text_color(rgb(self.foreground))
            .text_size(px(13.))
            .child(self.metric.label().to_string())
    }
}

fn theme_option(name: &str, colors: crate::terminal_protocol::TerminalTheme) -> AnyElement {
    let swatches = |range: std::ops::Range<usize>| {
        div()
            .flex()
            .gap_1()
            .children(range.map(|index| div().w(px(12.)).h(px(12.)).bg(rgb(colors.ansi[index]))))
    };
    div()
        .w_full()
        .min_h(px(52.))
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .rounded_sm()
        .border_2()
        .border_color(rgb(colors.background))
        .bg(rgb(colors.background))
        .text_color(rgb(colors.foreground))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(swatches(0..8))
                .child(swatches(8..16)),
        )
        .child(
            div()
                .font_family("Consolas")
                .text_size(px(13.))
                .child(name.to_string()),
        )
        .into_any_element()
}

fn setting_row(
    label: String,
    hint: Option<String>,
    control: impl IntoElement,
    palette: Palette,
) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_3()
        .min_h(px(52.))
        .py_2()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(160.))
                .gap_1()
                .text_size(px(13.))
                .child(label)
                .when_some(hint, |view, hint| {
                    view.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.muted))
                            .child(hint),
                    )
                }),
        )
        .child(
            div()
                .w(px(280.))
                .max_w_full()
                .flex_shrink_0()
                .ml_auto()
                .child(control),
        )
        .into_any_element()
}

#[derive(Clone, Copy)]
enum AppearanceSection {
    Theme,
    Font,
    Window,
    Background,
}

fn settings_page_action(page: usize, owner: WeakEntity<AppView>) -> AnyElement {
    let (id, icon, label) = match page {
        8 => (
            "reload-themes",
            IconName::RefreshCw,
            crate::t!("settings.reload_theme"),
        ),
        1 => (
            "reset-shortcuts",
            IconName::Undo2,
            crate::t!("settings.reset"),
        ),
        2 => (
            "reset-metrics",
            IconName::Undo2,
            crate::t!("settings.reset"),
        ),
        5 => (
            "settings-add-host",
            IconName::Plus,
            crate::t!("ws.add_host"),
        ),
        6 => ("refresh-keys", IconName::RefreshCw, crate::t!("ws.refresh")),
        _ => return div().into_any_element(),
    };
    let button = match page {
        8 => icon_button(id, icon, label),
        1 | 2 => Button::new(id).ghost().small().label(label),
        5 | 6 => Button::new(id).small().icon(icon).label(label),
        _ => unreachable!(),
    };
    button
        .on_click(move |_, window, cx| {
            let _ = owner.update(cx, |app, cx| match page {
                8 => app.reload_theme_file(window, cx),
                1 => {
                    app.keybindings.clear();
                    app.shortcut_map = shortcuts::Keymap::new(&app.keybindings);
                    app.settings_ui.recording = None;
                    app.settings_ui.notice = None;
                    app.save(cx);
                    cx.notify();
                }
                2 => {
                    app.metrics_config = metrics_config::Config::default();
                    app.sync_metrics(cx);
                    app.sync_latency_monitor(window, cx);
                    app.save(cx);
                    cx.notify();
                }
                5 => app.show_add(window, cx),
                6 => app.refresh_keys(cx),
                _ => {}
            });
        })
        .into_any_element()
}

pub(super) struct SettingsUi {
    pub page: usize,
    pub host_form: bool,
    keys: Vec<KeyInfo>,
    pub recording: Option<usize>,
    pub notice: Option<String>,
    pub(super) focus: FocusHandle,
    fonts: Entity<SelectState<SearchableVec<String>>>,
    light_themes: Entity<SelectState<SearchableVec<SchemeItem>>>,
    dark_themes: Entity<SelectState<SearchableVec<SchemeItem>>>,
    pub(super) theme_editor: Entity<EditorState>,
    editing_theme_id: Option<String>,
    pub(super) theme_edit_error: Option<String>,
    opacity: Entity<SliderState>,
    sidebar_opacity: Entity<SliderState>,
    window_width: Entity<InputState>,
    window_height: Entity<InputState>,
    window_size_error: bool,
    save_task: Option<Task<()>>,
}
impl SettingsUi {
    pub fn new(
        fonts: Vec<String>,
        family: &str,
        opacity: f32,
        sidebar_opacity: f32,
        initial_window_size: [f32; 2],
        themes: &ThemeFile,
        light_theme: &str,
        dark_theme: &str,
        window: &mut Window,
        cx: &mut Context<AppView>,
    ) -> Self {
        let fonts = cx.new(|cx| {
            let mut state =
                SelectState::new(SearchableVec::new(fonts), None, window, cx).searchable(true);
            state.set_selected_value(&family.to_string(), window, cx);
            state
        });
        let mut make_scheme_select = |id: &str, cx: &mut Context<AppView>| {
            cx.new(|cx| {
                let mut state =
                    SelectState::new(scheme_items(themes), None, window, cx).searchable(true);
                state.set_selected_value(&id.to_string(), window, cx);
                state
            })
        };
        let light_themes = make_scheme_select(light_theme, cx);
        let dark_themes = make_scheme_select(dark_theme, cx);
        Self {
            page: 0,
            host_form: false,
            keys: Vec::new(),
            recording: None,
            notice: None,
            focus: cx.focus_handle(),
            fonts,
            light_themes,
            dark_themes,
            window_width: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(format!("{}", initial_window_size[0] as u32))
            }),
            window_height: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(format!("{}", initial_window_size[1] as u32))
            }),
            window_size_error: false,
            theme_editor: cx.new(|cx| EditorState::new(window, cx).language("json")),
            editing_theme_id: None,
            theme_edit_error: None,
            save_task: None,
            opacity: cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(80.)
                    .step(1.)
                    .default_value((1. - opacity) * 100.)
            }),
            sidebar_opacity: cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(80.)
                    .step(1.)
                    .default_value((1. - sidebar_opacity) * 100.)
            }),
        }
    }
}

struct KeyInfo {
    path: PathBuf,
    fingerprint: String,
    public_key: Option<String>,
}

fn local_keys(hosts: &[Host]) -> Vec<KeyInfo> {
    let mut paths = std::collections::BTreeSet::new();
    if let Some(directory) = dirs::home_dir().map(|home| home.join(".ssh")) {
        if let Ok(entries) = std::fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|extension| extension == "pub") {
                    paths.insert(path.with_extension(""));
                }
            }
        }
    }
    paths.extend(hosts.iter().filter_map(|host| {
        host.config
            .as_ref()
            .and_then(|config| config.identity_file.clone())
    }));
    paths
        .into_iter()
        .filter_map(|path| {
            if !path.is_file() {
                return None;
            }
            let mut public_path = path.as_os_str().to_os_string();
            public_path.push(".pub");
            let public = russh::keys::load_public_key(PathBuf::from(public_path)).ok();
            Some(KeyInfo {
                path,
                fingerprint: public
                    .as_ref()
                    .map(|key| key.fingerprint(russh::keys::HashAlg::Sha256).to_string())
                    .unwrap_or_else(|| crate::t!("settings.public_key_missing").into_owned()),
                public_key: public.and_then(|key| key.to_openssh().ok()),
            })
        })
        .collect()
}

impl AppView {
    pub(super) fn refresh_keys(&mut self, cx: &mut Context<Self>) {
        self.settings_ui.keys = local_keys(&self.hosts);
        cx.notify();
    }

    fn assign_key(&mut self, host: usize, path: PathBuf, cx: &mut Context<Self>) {
        let Some(config) = self
            .hosts
            .get_mut(host)
            .and_then(|host| host.config.as_mut())
        else {
            return;
        };
        if config.identity_file.as_ref() == Some(&path) {
            return;
        }
        config.identity_file = Some(path);
        self.hosts[host].backend = None;
        self.hosts[host].views.clear();
        self.hosts[host].viewport = None;
        self.save(cx);
        if host == self.active {
            self.switch_host(host, cx);
        }
        cx.notify();
    }

    fn confirm_remove_host(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let cancel = crate::t!("ws.cancel").into_owned();
        let remove = crate::t!("settings.remove_host").into_owned();
        let title = crate::t!(
            "settings.remove_host_confirm",
            name = self.hosts[index].name.clone()
        )
        .into_owned();
        let receiver = window.prompt(
            PromptLevel::Warning,
            &title,
            None,
            &[cancel.as_str(), remove.as_str()],
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            if receiver.await == Ok(1) {
                let _ = view.update(cx, |app, cx| app.remove_host(index, cx));
            }
        })
        .detach();
    }

    fn host_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        div()
            .flex()
            .flex_col()
            .children(self.hosts.iter().enumerate().map(|(index, host)| {
                let detail = host
                    .config
                    .as_ref()
                    .map(|config| {
                        format!(
                            "{}@{}:{}",
                            config.user,
                            config.destination,
                            config.port.unwrap_or(22)
                        )
                    })
                    .unwrap_or_else(|| crate::t!("settings.local_host").into_owned());
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .min_h(px(48.))
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        Icon::new(if host.config.is_some() {
                            IconName::Server
                        } else {
                            IconName::Monitor
                        })
                        .size(px(16.))
                        .flex_shrink_0()
                        .text_color(rgb(p.muted)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .line_height(relative(1.25))
                            .child(host.name.clone())
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .line_height(relative(1.25))
                                    .text_color(rgb(p.muted))
                                    .truncate()
                                    .child(detail),
                            ),
                    )
                    .child(
                        icon_button(
                            ("settings-open-host", index),
                            IconName::ArrowUpRight,
                            crate::t!("settings.open_host"),
                        )
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.close_modal(window, cx);
                                app.switch_host(index, cx);
                            },
                        )),
                    )
                    .child(
                        icon_button(
                            ("settings-edit-host", index),
                            IconName::Pencil,
                            crate::t!("settings.edit"),
                        )
                        .on_click(
                            cx.listener(move |app, _, window, cx| app.edit_host(index, window, cx)),
                        ),
                    )
                    .when(index != 0, |row| {
                        row.child(
                            icon_button(
                                ("settings-remove-host", index),
                                IconName::Trash,
                                crate::t!("settings.remove_host"),
                            )
                            .on_click(cx.listener(
                                move |app, _, window, cx| {
                                    app.confirm_remove_host(index, window, cx)
                                },
                            )),
                        )
                    })
            }))
            .into_any_element()
    }

    fn key_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .when(self.settings_ui.keys.is_empty(), |view| {
                view.child(
                    div()
                        .text_color(rgb(p.muted))
                        .child(crate::t!("settings.no_keys")),
                )
            })
            .children(
                self.settings_ui
                    .keys
                    .iter()
                    .enumerate()
                    .map(|(index, key)| {
                        let public_key = key.public_key.clone();
                        let path = key.path.clone();
                        let hosts: Vec<_> = self
                            .hosts
                            .iter()
                            .enumerate()
                            .skip(1)
                            .map(|(host_index, host)| {
                                (
                                    host_index,
                                    host.name.clone(),
                                    host.config
                                        .as_ref()
                                        .and_then(|config| config.identity_file.as_ref())
                                        == Some(&key.path),
                                )
                            })
                            .collect();
                        let assigned = hosts
                            .iter()
                            .filter(|(_, _, selected)| *selected)
                            .map(|(_, name, _)| name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        let owner = cx.entity().downgrade();
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .min_h(px(52.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .line_height(relative(1.25))
                                    .child(div().truncate().child(key.path.display().to_string()))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .line_height(relative(1.25))
                                            .text_color(rgb(p.muted))
                                            .child(key.fingerprint.clone()),
                                    )
                                    .when(!assigned.is_empty(), |details| {
                                        details.child(
                                            div()
                                                .text_size(px(11.))
                                                .line_height(relative(1.25))
                                                .text_color(rgb(p.muted))
                                                .truncate()
                                                .child(assigned),
                                        )
                                    }),
                            )
                            .child(
                                icon_button(
                                    ("copy-public-key", index),
                                    IconName::Copy,
                                    crate::t!("settings.copy_public_key"),
                                )
                                .disabled(public_key.is_none())
                                .on_click(cx.listener(
                                    move |_, _, _, cx| {
                                        if let Some(public_key) = &public_key {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                public_key.clone(),
                                            ));
                                        }
                                    },
                                )),
                            )
                            .child(
                                Button::new(("assign-key", index))
                                    .small()
                                    .label(crate::t!("settings.assign_key"))
                                    .disabled(hosts.is_empty())
                                    .dropdown_caret(true)
                                    .dropdown_menu(move |mut menu, _, _| {
                                        for (host, name, selected) in &hosts {
                                            let host = *host;
                                            let path = path.clone();
                                            let owner = owner.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(name.clone())
                                                    .checked(*selected)
                                                    .on_click(move |_, _, cx| {
                                                        let _ = owner.update(cx, |app, cx| {
                                                            app.assign_key(host, path.clone(), cx)
                                                        });
                                                    }),
                                            );
                                        }
                                        menu
                                    }),
                            )
                    }),
            )
            .into_any_element()
    }
    pub(super) fn theme_path() -> PathBuf {
        Preferences::path().with_file_name("theme.json")
    }

    pub(super) fn legacy_theme_path() -> PathBuf {
        Preferences::path().with_file_name("terminal-theme.json")
    }

    pub(super) fn reload_theme_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match ThemeFile::load(&Self::theme_path()) {
            Ok(themes) => {
                self.themes = themes;
                self.theme_error = None;
                if self.repair_scheme_slots(window, cx) {
                    self.save(cx);
                }
                self.apply_appearance(window, cx);
            }
            Err(error) => self.theme_error = Some(error.to_string()),
        }
        cx.notify();
    }

    fn sync_scheme_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (state, selected) in [
            (&self.settings_ui.light_themes, &self.light_theme),
            (&self.settings_ui.dark_themes, &self.dark_theme),
        ] {
            state.update(cx, |state, cx| {
                state.set_items(scheme_items(&self.themes), window, cx);
                state.set_selected_value(selected, window, cx);
            });
        }
    }

    fn repair_scheme_slots(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let current = Preferences {
            light_theme: Some(self.light_theme.clone()),
            dark_theme: Some(self.dark_theme.clone()),
            ..Preferences::default()
        };
        let (light, dark) = theme_slots(&self.themes, &current);
        let changed = self.light_theme != light || self.dark_theme != dark;
        self.light_theme = light;
        self.dark_theme = dark;
        self.sync_scheme_selects(window, cx);
        changed
    }

    pub(super) fn select_theme(
        &mut self,
        mode: ThemeMode,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.themes.selected(id).is_none() {
            return;
        }
        match mode {
            ThemeMode::Light => self.light_theme = id.to_string(),
            ThemeMode::Dark => self.dark_theme = id.to_string(),
        }
        self.sync_scheme_selects(window, cx);
        self.apply_appearance(window, cx);
        self.save(cx);
    }

    pub(super) fn open_theme_editor(
        &mut self,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = id
            .as_deref()
            .and_then(|id| self.themes.selected(id))
            .cloned()
            .unwrap_or_else(|| {
                let id = self.themes.next_id();
                let base = self
                    .themes
                    .selected(&active_theme_id(
                        &self.themes,
                        &self.light_theme,
                        &self.dark_theme,
                        self.appearance,
                        window.appearance(),
                    ))
                    .or_else(|| self.themes.selected(self.themes.fallback_id()))
                    .expect("validated theme file must contain a theme");
                let mut theme = ThemeDefinition::from_theme(base, id.clone());
                theme.name = format!(
                    "{} {}",
                    crate::t!("settings.new_theme_name"),
                    id.rsplit('-').next().unwrap_or("1")
                );
                theme
            });
        let text = serde_json::to_string_pretty(&theme).expect("theme template is serializable");
        self.settings_ui
            .theme_editor
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
        self.settings_ui.editing_theme_id = id;
        self.settings_ui.theme_edit_error = None;
        let owner = cx.entity();
        let body = cx.new(|cx| ModalContent {
            owner: owner.downgrade(),
            kind: ModalKind::ThemeEditor,
            _subscription: cx.observe(&owner, |_, _, cx| cx.notify()),
        });
        window.open_dialog(cx, move |dialog, window, cx| {
            let height = 460_f32.min((f32::from(window.viewport_size().height) - 180.).max(180.));
            workspace_dialog(dialog, cx)
                .title(crate::t!("settings.edit_theme"))
                .width(px(640.))
                .margin_top(((window.viewport_size().height - px(height + 80.)) / 2.).max(px(24.)))
                .child(div().h(px(height)).min_h_0().child(body.clone()))
        });
        self.settings_ui
            .theme_editor
            .update(cx, |editor, cx| editor.focus(window, cx));
    }

    pub(super) fn save_theme_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| -> anyhow::Result<(ThemeFile, String)> {
            anyhow::ensure!(
                ThemeFile::load(&Self::theme_path())? == self.themes,
                "{}",
                crate::t!("settings.theme_file_changed")
            );
            let text = self.settings_ui.theme_editor.read(cx).value();
            let mut theme: ThemeDefinition = serde_json::from_str(&text)?;
            theme.ensure_interface();
            let mut file = self.themes.clone();
            if let Some(old_id) = &self.settings_ui.editing_theme_id {
                let index = file
                    .themes
                    .iter()
                    .position(|item| item.id == *old_id)
                    .ok_or_else(|| anyhow::anyhow!("{}", crate::t!("settings.theme_missing")))?;
                file.themes[index] = theme.clone();
            } else {
                file.themes.push(theme.clone());
            }
            ThemeFile::parse(&serde_json::to_vec(&file)?)?;
            file.write(&Self::theme_path())?;
            Ok((file, theme.id))
        })();
        match result {
            Ok((file, id)) => {
                self.themes = file;
                if let Some(old_id) = &self.settings_ui.editing_theme_id {
                    if self.light_theme == *old_id {
                        self.light_theme = id.clone();
                    }
                    if self.dark_theme == *old_id {
                        self.dark_theme = id.clone();
                    }
                }
                self.theme_error = None;
                self.sync_scheme_selects(window, cx);
                self.apply_appearance(window, cx);
                self.save(cx);
                window.close_dialog(cx);
            }
            Err(error) => {
                self.settings_ui.theme_edit_error = Some(error.to_string());
                cx.notify();
            }
        }
    }

    fn confirm_delete_theme(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let cancel = crate::t!("explorer.cancel").into_owned();
        let delete = crate::t!("settings.delete_theme").into_owned();
        let title = crate::t!("settings.delete_theme_title").into_owned();
        let receiver = window.prompt(
            PromptLevel::Warning,
            &title,
            None,
            &[cancel.as_str(), delete.as_str()],
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            if receiver.await == Ok(1) {
                let _ = view.update_in(cx, |app, window, cx| app.delete_theme(&id, window, cx));
            }
        })
        .detach();
    }

    pub(super) fn delete_theme(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut file = self.themes.clone();
        if file.themes.len() <= 1 {
            self.theme_error = Some(crate::t!("settings.theme_last_required").to_string());
            cx.notify();
            return;
        }
        file.themes.retain(|theme| theme.id != id);
        let result = ThemeFile::load(&Self::theme_path()).and_then(|disk| {
            anyhow::ensure!(
                disk == self.themes,
                "{}",
                crate::t!("settings.theme_file_changed")
            );
            file.write(&Self::theme_path())
        });
        match result {
            Ok(()) => {
                self.themes = file;
                self.theme_error = None;
                self.repair_scheme_slots(window, cx);
                self.apply_appearance(window, cx);
                self.save(cx);
            }
            Err(error) => {
                self.theme_error = Some(error.to_string());
                cx.notify();
            }
        }
    }

    pub(super) fn theme_editor_view(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(rgb(self.palette.border))
                    .child(gpui_kit::base::input::Editor::new(
                        &self.settings_ui.theme_editor,
                    )),
            )
            .when_some(self.settings_ui.theme_edit_error.clone(), |view, error| {
                view.child(
                    div()
                        .min_w_0()
                        .max_h(px(72.))
                        .overflow_y_scrollbar()
                        .text_size(px(12.))
                        .text_color(rgb(self.palette.error))
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-theme-edit")
                            .label(crate::t!("explorer.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("save-theme-edit")
                            .primary()
                            .label(crate::t!("settings.save_theme"))
                            .on_click(
                                cx.listener(|app, _, window, cx| app.save_theme_editor(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn scheme_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(self.themes.themes.iter().enumerate().map(|(index, theme)| {
                let edit_id = theme.id.clone();
                let delete_id = theme.id.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(theme_option(&theme.name, theme.colors())),
                    )
                    .child(
                        icon_button(
                            ("edit-theme", index),
                            IconName::Pencil,
                            crate::t!("settings.edit_theme"),
                        )
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.open_theme_editor(Some(edit_id.clone()), window, cx)
                            },
                        )),
                    )
                    .child(
                        icon_button(
                            ("delete-theme", index),
                            IconName::Trash,
                            crate::t!("settings.delete_theme"),
                        )
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.confirm_delete_theme(delete_id.clone(), window, cx)
                            },
                        )),
                    )
            }))
            .child(
                Button::new("add-theme")
                    .small()
                    .icon(IconName::Plus)
                    .label(crate::t!("settings.add_theme"))
                    .on_click(
                        cx.listener(|app, _, window, cx| app.open_theme_editor(None, window, cx)),
                    ),
            )
            .when_some(self.theme_error.clone(), |view, error| {
                view.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(self.palette.error))
                        .child(error),
                )
            })
            .into_any_element()
    }

    fn theme_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(setting_row(
                crate::t!("settings.language").to_string(),
                Some(crate::t!("settings.language_hint").to_string()),
                div().flex().justify_end().gap_2().children(
                    crate::i18n::Language::ALL
                        .iter()
                        .enumerate()
                        .map(|(index, &language)| {
                            Button::new(("language", index))
                                .small()
                                .label(language.label())
                                .when(self.language == language, |b| b.primary())
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.set_language(language, cx);
                                }))
                        }),
                ),
                p,
            ))
            .child(setting_row(
                crate::t!("settings.interface_theme").to_string(),
                Some(crate::t!("settings.appearance_hint").to_string()),
                div().flex().justify_end().gap_2().children(
                    [
                        (
                            "appearance-system",
                            "settings.appearance_system",
                            Appearance::System,
                        ),
                        (
                            "appearance-light",
                            "settings.appearance_light",
                            Appearance::Light,
                        ),
                        (
                            "appearance-dark",
                            "settings.appearance_dark",
                            Appearance::Dark,
                        ),
                    ]
                    .into_iter()
                    .map(|(id, label, appearance)| {
                        Button::new(id)
                            .small()
                            .label(crate::t!(label))
                            .when(self.appearance == appearance, |button| button.primary())
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.set_appearance(appearance, window, cx);
                            }))
                    }),
                ),
                p,
            ))
            .child(setting_row(
                crate::t!("settings.light_theme").to_string(),
                None,
                Select::new(&self.settings_ui.light_themes)
                    .search_placeholder(crate::t!("settings.search_theme"))
                    .w_full(),
                p,
            ))
            .child(setting_row(
                crate::t!("settings.dark_theme").to_string(),
                None,
                Select::new(&self.settings_ui.dark_themes)
                    .search_placeholder(crate::t!("settings.search_theme"))
                    .w_full(),
                p,
            ))
            .into_any_element()
    }

    fn set_window_size(
        &mut self,
        dimensions: [f32; 2],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.window_size = Some(dimensions);
        self.settings_ui.window_width.update(cx, |input, cx| {
            input.set_value(format!("{}", dimensions[0] as u32), window, cx)
        });
        self.settings_ui.window_height.update(cx, |input, cx| {
            input.set_value(format!("{}", dimensions[1] as u32), window, cx)
        });
        self.settings_ui.window_size_error = false;
        self.save(cx);
        cx.notify();
    }
    fn apply_window_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let parse = |input: &Entity<InputState>, minimum| {
            input
                .read(cx)
                .value()
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|value| (minimum..=8192).contains(value))
                .map(|value| value as f32)
        };
        match (
            parse(&self.settings_ui.window_width, 860),
            parse(&self.settings_ui.window_height, 520),
        ) {
            (Some(width), Some(height)) => self.set_window_size([width, height], window, cx),
            _ => {
                self.settings_ui.window_size_error = true;
                cx.notify();
            }
        }
    }
    pub(super) fn set_appearance(
        &mut self,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance = appearance;
        self.apply_appearance(window, cx);
        self.save(cx);
    }
    pub(super) fn set_language(&mut self, language: crate::i18n::Language, cx: &mut Context<Self>) {
        self.language = language;
        crate::i18n::set_language(language);
        self.save(cx);
        cx.notify();
    }
    pub(super) fn set_terminal_theme(
        &mut self,
        theme: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_theme(self.appearance.mode(window.appearance()), theme, window, cx);
    }
    pub(super) fn set_ligatures(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.ligatures = enabled;
        for host in &self.hosts {
            for view in host.views.values() {
                view.update(cx, |view, cx| {
                    view.ligatures = enabled;
                    cx.notify();
                });
            }
        }
        self.save(cx);
        cx.notify();
    }
    pub(super) fn subscribe_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            &self.settings_ui.light_themes,
            window,
            |app, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(id)) = event {
                    app.select_theme(ThemeMode::Light, id, window, cx);
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &self.settings_ui.dark_themes,
            window,
            |app, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(id)) = event {
                    app.select_theme(ThemeMode::Dark, id, window, cx);
                }
            },
        )
        .detach();
        cx.subscribe_in(&self.settings_ui.fonts, window, |app, _, event, _, cx| {
            if let SelectEvent::Confirm(Some(family)) = event {
                app.set_font_family(family.clone(), cx);
            }
        })
        .detach();
        for input in [
            &self.settings_ui.window_width,
            &self.settings_ui.window_height,
        ] {
            cx.subscribe_in(input, window, |app, _, event, window, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    app.apply_window_size(window, cx);
                }
            })
            .detach();
        }
        cx.subscribe_in(
            &self.settings_ui.opacity,
            window,
            |app, _, event, window, cx| match event {
                SliderEvent::Change(value) => {
                    app.background_opacity = 1. - value.end().clamp(0., 80.) / 100.;
                    app.apply_opacity(window, cx);
                    app.settings_ui.save_task = Some(cx.spawn(async move |app, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(150))
                            .await;
                        let _ = app.update(cx, |app, cx| app.save(cx));
                    }));
                }
                SliderEvent::Release(_) => app.save(cx),
            },
        )
        .detach();
        cx.subscribe_in(
            &self.settings_ui.sidebar_opacity,
            window,
            |app, _, event, window, cx| match event {
                SliderEvent::Change(value) => {
                    app.sidebar_opacity = 1. - value.end().clamp(0., 80.) / 100.;
                    app.apply_opacity(window, cx);
                    app.settings_ui.save_task = Some(cx.spawn(async move |app, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(150))
                            .await;
                        let _ = app.update(cx, |app, cx| app.save(cx));
                    }));
                }
                SliderEvent::Release(_) => app.save(cx),
            },
        )
        .detach();
    }
    pub(super) fn set_font_family(&mut self, family: String, cx: &mut Context<Self>) {
        self.font_family = family;
        for host in &mut self.hosts {
            host.viewport = None;
            for view in host.views.values() {
                view.update(cx, |view, cx| {
                    view.font_family = self.font_family.clone();
                    cx.notify();
                });
            }
        }
        self.save(cx);
        cx.notify();
    }

    pub(super) fn set_line_height(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.line_height_scale = factor.into();
        for host in &mut self.hosts {
            host.viewport = None;
            for view in host.views.values() {
                view.update(cx, |view, cx| {
                    view.line_height_scale = self.line_height_scale;
                    cx.notify();
                });
            }
        }
        self.save(cx);
        cx.notify();
    }
    pub(super) fn background_appearance(&self) -> WindowBackgroundAppearance {
        let sidebar_glass = !self.zen && !self.sidebar_collapsed && self.sidebar_opacity < 1.;
        if sidebar_glass || (self.acrylic_background && self.background_opacity < 1.) {
            WindowBackgroundAppearance::Blurred
        } else if self.background_opacity < 1. {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        }
    }

    pub(super) fn apply_opacity(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_background_appearance(self.background_appearance());
        window.refresh();
        cx.notify();
    }
    pub(super) fn record_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = &event.keystroke;
        cx.stop_propagation();
        window.prevent_default();
        if key.key == "escape" {
            self.settings_ui.recording = None;
            self.settings_ui.notice = None;
        } else if !event.is_held
            && !matches!(
                key.key.as_str(),
                "control" | "ctrl" | "alt" | "shift" | "super" | "meta"
            )
        {
            if let Some(index) = self.settings_ui.recording {
                match shortcuts::assign(index, key, &mut self.keybindings) {
                    Ok(()) => {
                        self.shortcut_map = shortcuts::Keymap::new(&self.keybindings);
                        self.settings_ui.recording = None;
                        self.settings_ui.notice = None;
                        self.save(cx);
                    }
                    Err(message) => self.settings_ui.notice = Some(message),
                }
            }
        }
        cx.notify();
    }
    fn font_settings(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(setting_row(
                crate::t!("settings.font_family").to_string(),
                Some(crate::t!("settings.font_mono_hint").to_string()),
                Select::new(&self.settings_ui.fonts)
                    .search_placeholder(crate::t!("settings.font_search"))
                    .w_full(),
                p,
            ))
            .child(setting_row(
                crate::t!("settings.font_size").to_string(),
                None,
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .justify_end()
                    .child(
                        icon_button(
                            "font-down",
                            IconName::Minus,
                            crate::t!("settings.font_smaller"),
                        )
                        .disabled(self.font_size <= 10.)
                        .on_click(cx.listener(|app, _, _, cx| app.font(-1., cx))),
                    )
                    .child(
                        div()
                            .w(px(54.))
                            .text_center()
                            .child(format!("{} px", self.font_size as u32)),
                    )
                    .child(
                        icon_button("font-up", IconName::Plus, crate::t!("settings.font_larger"))
                            .disabled(self.font_size >= 24.)
                            .on_click(cx.listener(|app, _, _, cx| app.font(1., cx))),
                    ),
                p,
            ))
            .child(setting_row(
                crate::t!("settings.line_height").to_string(),
                Some(crate::t!("settings.line_height_hint").to_string()),
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_3()
                    .child(
                        icon_button(
                            "line-height-down",
                            IconName::Minus,
                            crate::t!("settings.line_height_less"),
                        )
                        .disabled(self.line_height_scale.factor() <= 1.)
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.set_line_height(
                                (app.line_height_scale.factor() * 10. - 1.).round() / 10.,
                                cx,
                            );
                        })),
                    )
                    .child(
                        div()
                            .w(px(54.))
                            .text_center()
                            .child(format!("{:.1}×", self.line_height_scale.factor())),
                    )
                    .child(
                        icon_button(
                            "line-height-up",
                            IconName::Plus,
                            crate::t!("settings.line_height_more"),
                        )
                        .disabled(self.line_height_scale.factor() >= 2.)
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.set_line_height(
                                (app.line_height_scale.factor() * 10. + 1.).round() / 10.,
                                cx,
                            );
                        })),
                    ),
                p,
            ))
            .child(setting_row(
                crate::t!("settings.ligatures").to_string(),
                Some(crate::t!("settings.ligatures_hint").to_string()),
                div().flex().justify_end().child(
                    Checkbox::new("font-ligatures")
                        .checked(self.ligatures)
                        .on_click(cx.listener(|app, enabled, _, cx| {
                            app.set_ligatures(*enabled, cx);
                        })),
                ),
                p,
            ))
            .child(
                div()
                    .p_4()
                    .rounded_md()
                    .bg(rgb(p.terminal))
                    .font_family(self.font_family.clone())
                    .text_size(px(self.font_size))
                    .line_height(px(self.line_height_scale.measure(
                        window,
                        &self.font_family,
                        self.font_size,
                    )))
                    .child("Aa Bb 0123456789  {} [] ()")
                    .child(div().mt_2().child(crate::t!("settings.font_preview"))),
            )
            .into_any_element()
    }
    fn metrics_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(
                [metrics_config::Side::Left, metrics_config::Side::Right]
                    .into_iter()
                    .map(|side| {
                        let accent = self.palette.accent;
                        let title = match side {
                            metrics_config::Side::Left => crate::t!("settings.metrics_left"),
                            metrics_config::Side::Right => crate::t!("settings.metrics_right"),
                        };
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .when(side == metrics_config::Side::Right, |view| {
                                view.mt_3()
                                    .pt_3()
                                    .border_t_1()
                                    .border_color(rgb(self.palette.border))
                            })
                            .child(
                                div()
                                    .id(match side {
                                        metrics_config::Side::Left => "metric-zone-left",
                                        metrics_config::Side::Right => "metric-zone-right",
                                    })
                                    .min_h(px(38.))
                                    .px_4()
                                    .flex()
                                    .items_center()
                                    .border_b_1()
                                    .border_color(rgb(self.palette.border))
                                    .text_color(rgb(self.palette.muted))
                                    .child(title)
                                    .drag_over::<MetricDrag>(move |style, _, _, _| {
                                        style.border_color(rgb(accent))
                                    })
                                    .on_drop(cx.listener(move |app, drag: &MetricDrag, _, cx| {
                                        if app.metrics_config.move_to_side(drag.metric, side) {
                                            app.save(cx);
                                            cx.notify();
                                        }
                                    })),
                            )
                            .children(
                                self.metrics_config
                                    .0
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, item)| item.side() == side)
                                    .map(|(index, item)| self.metric_settings_row(index, item, cx)),
                            )
                    }),
            )
            .into_any_element()
    }
    fn metric_settings_row(
        &self,
        index: usize,
        item: &metrics_config::Item,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let metric = item.metric;
        let order: Vec<_> = self
            .metrics_config
            .0
            .iter()
            .map(|item| item.metric)
            .collect();
        let accent = self.palette.muted;
        div()
            .id(("metric-row", metric as usize))
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .min_h(px(48.))
            .border_1()
            .border_color(rgba(0))
            .drag_over::<MetricDrag>(move |style, drag, _, _| {
                if drag.metric == metric {
                    return style;
                }
                let from = order
                    .iter()
                    .position(|metric| *metric == drag.metric)
                    .unwrap_or(index);
                if from < index {
                    style.border_t_0().border_b_2().border_color(rgb(accent))
                } else {
                    style.border_t_2().border_b_0().border_color(rgb(accent))
                }
            })
            .on_drop(cx.listener(move |app, drag: &MetricDrag, _, cx| {
                if app.metrics_config.move_to(drag.metric, metric) {
                    app.save(cx);
                    cx.notify();
                }
            }))
            .child(
                div().flex_1().child(
                    Checkbox::new(("metric-enabled", metric as usize))
                        .small()
                        .label(metric.label().into_owned())
                        .checked(item.enabled)
                        .on_click(cx.listener(move |app, checked, window, cx| {
                            if let Some(item) = app
                                .metrics_config
                                .0
                                .iter_mut()
                                .find(|item| item.metric == metric)
                            {
                                item.enabled = *checked;
                            }
                            app.sync_metrics(cx);
                            app.sync_latency_monitor(window, cx);
                            app.save(cx);
                            cx.notify();
                        })),
                ),
            )
            .child(
                div()
                    .id(("metric-grip", metric as usize))
                    .w(px(28.))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(CursorStyle::OpenHand)
                    .text_color(rgb(self.palette.muted))
                    .child(Icon::new(IconName::GripVertical).size(px(14.)))
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            crate::t!("settings.metrics_drag").into_owned(),
                        )
                        .build(window, cx)
                    })
                    .on_drag(
                        MetricDrag {
                            metric,
                            background: self.palette.panel,
                            foreground: self.palette.text,
                        },
                        |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| drag.clone())
                        },
                    ),
            )
            .into_any_element()
    }
    fn component_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let owner = cx.entity().downgrade();
        let pages = [
            (
                0,
                crate::t!("settings.appearance").into_owned(),
                IconName::Sun,
            ),
            (
                8,
                crate::t!("settings.terminal_theme").into_owned(),
                IconName::Pencil,
            ),
            (
                1,
                crate::t!("settings.shortcuts").into_owned(),
                IconName::Command,
            ),
            (
                2,
                crate::t!("settings.metrics").into_owned(),
                IconName::Settings,
            ),
            (
                5,
                crate::t!("settings.hosts").into_owned(),
                IconName::Server,
            ),
            (6, crate::t!("settings.keys").into_owned(), IconName::Key),
            #[cfg(windows)]
            (
                7,
                crate::t!("updates.title").into_owned(),
                IconName::RefreshCw,
            ),
        ];
        let selected_page = pages
            .iter()
            .position(|(page, _, _)| *page == self.settings_ui.page)
            .unwrap_or(0);
        Settings::new(SharedString::from(format!(
            "workspace-settings-{selected_page}"
        )))
        .small()
        .sidebar_width(px(190.))
        .sidebar_size_range(px(168.)..px(248.))
        .default_selected_index(SelectIndex {
            page_ix: selected_page,
            group_ix: None,
        })
        .pages(pages.into_iter().map(|(page, label, icon)| {
            let owner = owner.clone();
            if page == 0 {
                let sections = [
                    (
                        AppearanceSection::Theme,
                        crate::t!("settings.theme_settings"),
                    ),
                    (AppearanceSection::Font, crate::t!("settings.font")),
                    (AppearanceSection::Window, crate::t!("settings.window")),
                    (
                        AppearanceSection::Background,
                        crate::t!("settings.background"),
                    ),
                ];
                return SettingPage::new(label.clone()).icon(icon).groups(
                    sections.into_iter().map(|(section, section_label)| {
                        let owner = owner.clone();
                        let section_label = section_label.into_owned();
                        let keywords = [label.clone(), section_label.clone()];
                        SettingGroup::new().title(section_label).item(
                            SettingItem::render(move |_, window, cx| {
                                owner
                                    .update(cx, |app, cx| {
                                        app.settings_content(page, Some(section), window, cx)
                                    })
                                    .unwrap_or_else(|_| div().into_any_element())
                            })
                            .keywords(keywords),
                        )
                    }),
                );
            }
            let keywords = [label.clone()];
            SettingPage::new(label).icon(icon).group(
                SettingGroup::new().item(
                    SettingItem::render(move |_, window, cx| {
                        owner
                            .update(cx, |app, cx| app.settings_content(page, None, window, cx))
                            .unwrap_or_else(|_| div().into_any_element())
                    })
                    .keywords(keywords),
                ),
            )
        }))
        .into_any_element()
    }
    pub(super) fn settings_view(&self, _: &Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .min_h_0()
            .track_focus(&self.settings_ui.focus)
            .child(self.component_settings(cx))
            .into_any_element()
    }

    fn settings_content(
        &self,
        page: usize,
        appearance_section: Option<AppearanceSection>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let content =
            match page {
                #[cfg(windows)]
                7 => self.update_settings(cx),
                6 => self.key_settings(cx),
                5 => self.host_settings(cx),
                8 => self.scheme_settings(cx),
                2 => self.metrics_settings(cx),
                1 => div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when_some(self.settings_ui.notice.clone(), |v, notice| {
                        v.child(div().text_color(rgb(p.error)).child(notice))
                    })
                    .children(
                        shortcuts::BINDINGS
                            .iter()
                            .enumerate()
                            .map(|(index, binding)| {
                                let recording = self.settings_ui.recording == Some(index);
                                let label = if recording {
                                    crate::t!("settings.shortcuts_recording").to_string()
                                } else {
                                    shortcuts::keys(binding, &self.keybindings)
                                        .iter()
                                        .map(|s| shortcuts::display(s))
                                        .collect::<Vec<_>>()
                                        .join(" / ")
                                };
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .py_2()
                                    .min_h(px(48.))
                                    .child(div().flex_1().child(binding.label()))
                                    .child(
                                        Button::new(("shortcut", index))
                                            .small()
                                            .label(label)
                                            .when(recording, |b| b.primary())
                                            .on_click(cx.listener(move |app, _, window, cx| {
                                                app.settings_ui.recording = Some(index);
                                                app.settings_ui.notice = None;
                                                window.focus(&app.settings_ui.focus, cx);
                                                cx.notify();
                                            })),
                                    )
                            }),
                    )
                    .into_any_element(),
                _ => div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(
                        matches!(appearance_section, Some(AppearanceSection::Theme)),
                        |view| view.child(self.theme_settings(cx)),
                    )
                    .when(
                        matches!(appearance_section, Some(AppearanceSection::Font)),
                        |view| view.child(self.font_settings(window, cx)),
                    )
                    .when(
                        matches!(appearance_section, Some(AppearanceSection::Window)),
                        |view| {
                            view.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(setting_row(
                                        crate::t!("settings.initial_window_size").to_string(),
                                        None,
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_end()
                                            .gap_2()
                                            .child(crate::t!("settings.window_width"))
                                            .child(
                                                Input::new(&self.settings_ui.window_width)
                                                    .small()
                                                    .w(px(76.)),
                                            )
                                            .child(crate::t!("settings.window_height"))
                                            .child(
                                                Input::new(&self.settings_ui.window_height)
                                                    .small()
                                                    .w(px(76.)),
                                            ),
                                        p,
                                    ))
                                    .when(self.settings_ui.window_size_error, |view| {
                                        view.child(
                                            div()
                                                .text_color(rgb(p.error))
                                                .child(crate::t!("settings.window_size_invalid")),
                                        )
                                    }),
                            )
                            .child(setting_row(
                                crate::t!("settings.show_top_title").to_string(),
                                None,
                                div().flex().justify_end().child(
                                    Checkbox::new("show-top-title")
                                        .checked(self.show_top_title)
                                        .on_click(cx.listener(|app, enabled, _, cx| {
                                            app.show_top_title = *enabled;
                                            app.save(cx);
                                            cx.notify();
                                        })),
                                ),
                                p,
                            ))
                            .child(setting_row(
                                crate::t!("settings.show_status_bar").to_string(),
                                None,
                                div().flex().justify_end().child(
                                    Checkbox::new("show-status-bar")
                                        .checked(self.show_status_bar)
                                        .on_click(cx.listener(|app, enabled, window, cx| {
                                            app.show_status_bar = *enabled;
                                            app.sync_latency_monitor(window, cx);
                                            app.save(cx);
                                            cx.notify();
                                        })),
                                ),
                                p,
                            ))
                        },
                    )
                    .when(
                        matches!(appearance_section, Some(AppearanceSection::Background)),
                        |view| {
                            view.child(setting_row(
                                crate::t!("settings.opacity").to_string(),
                                Some(crate::t!("settings.opacity_hint").to_string()),
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .child(Slider::new(&self.settings_ui.opacity).w_full()),
                                    )
                                    .child(format!(
                                        "{}%",
                                        ((1. - self.background_opacity) * 100.).round() as u32
                                    )),
                                p,
                            ))
                            .child(setting_row(
                                crate::t!("settings.sidebar_opacity").to_string(),
                                None,
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().flex_1().child(
                                        Slider::new(&self.settings_ui.sidebar_opacity).w_full(),
                                    ))
                                    .child(format!(
                                        "{}%",
                                        ((1. - self.sidebar_opacity) * 100.).round() as u32
                                    )),
                                p,
                            ))
                            .child(setting_row(
                                crate::t!("settings.acrylic").to_string(),
                                Some(crate::t!("settings.acrylic_hint").to_string()),
                                div().flex().justify_end().child(
                                    Checkbox::new("acrylic-background")
                                        .checked(self.acrylic_background)
                                        .on_click(cx.listener(|app, enabled, window, cx| {
                                            app.acrylic_background = *enabled;
                                            app.apply_opacity(window, cx);
                                            app.save(cx);
                                        })),
                                ),
                                p,
                            ))
                        },
                    )
                    .into_any_element(),
            };
        let description = match page {
            8 => Some(crate::t!("settings.theme_description")),
            1 => Some(crate::t!("settings.shortcuts_hint")),
            2 => Some(crate::t!("settings.metrics_hint")),
            5 => Some(crate::t!("settings.hosts_description")),
            6 => Some(crate::t!("settings.keys_description")),
            _ => None,
        };
        if let Some(description) = description {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .w_full()
                        .min_h(px(32.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(rgb(p.muted))
                                .child(description),
                        )
                        .child(settings_page_action(page, cx.entity().downgrade())),
                )
                .child(content)
                .into_any_element()
        } else {
            content
        }
    }
}
