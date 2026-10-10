use crate::appearance::PaletteTerminalTheme;
use crate::{
    appearance::{Appearance, Palette},
    backend::{
        Action, Backend, Connection, LocalBackend, PANE_GAP, PANE_PADDING, PaneBounds,
        PixelViewport, Snapshot, SplitAxis,
    },
    host::HostDefinition,
    shortcuts::{self, Shortcut},
    terminal::validate_destination,
    terminal_theme::{self, ThemeDefinition, ThemeFile},
    terminal_view::TerminalView,
    tmux_client::TmuxClient,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Disableable, Icon, Root, Sizable, Theme, ThemeMode, WindowExt,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    command::{Command, CommandGroup, CommandItem, CommandState},
    form::{Field, Form},
    input::{EditorState, Input, InputEvent, InputState},
    kbd::Kbd,
    menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
    notification::Notification,
    scroll::ScrollableElement,
    status_bar::StatusBar,
};
use gpui_kit::{prelude::*, *};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Duration,
};
mod directories;
mod settings;
#[cfg(test)]
mod theme_tests;
use settings::SettingsUi;
use tshell_explorer::{self as explorer, Explorer};
use tshell_git::{GitView, git, selection as git_selection, view as git_view};
mod host_metrics;
mod launcher;
mod metrics_config;
mod metrics_transport;
#[cfg(test)]
mod preferences_tests;
use tshell_explorer::preview;
use tshell_host::sftp as remote_files;
mod sessions;
mod ssh_auth;
use tshell_ui::controls::{self as style, workspace_dialog};
mod docker;
mod terminal_actions;
mod tools;
mod transfer_queue;
#[cfg(windows)]
mod updates;
use tshell_explorer::workbench;

const SIDEBAR_WIDTH: f32 = 212.;
const WINDOW_CONTROL_WIDTH: f32 = 38.;
const WINDOW_CONTROLS_WIDTH: f32 = WINDOW_CONTROL_WIDTH * 3.;
const CHROME_BAR_HEIGHT: f32 = 28.;
pub(crate) const DEFAULT_WINDOW_SIZE: [f32; 2] = [1320., 840.];

#[cfg(debug_assertions)]
mod feature_check;
#[cfg(debug_assertions)]
mod ssh_ui_check;
#[cfg(debug_assertions)]
mod ui_check;
#[cfg(debug_assertions)]
pub(crate) use ui_check::run_ui_check;

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    line_height_scale: crate::terminal_metrics::LineHeight,
    ligatures: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_theme: Option<String>,
    light_theme: Option<String>,
    dark_theme: Option<String>,
    #[serde(rename = "terminal_theme", default, skip_serializing)]
    legacy_terminal_theme: Option<String>,
    #[serde(rename = "selected_custom_theme", default, skip_serializing)]
    legacy_custom_theme: Option<String>,
    #[serde(rename = "custom_theme_active", default, skip_serializing)]
    legacy_custom_theme_active: bool,
    font_size: f32,
    font_family: String,
    background_opacity: Option<f32>,
    sidebar_opacity: Option<f32>,
    acrylic_background: bool,
    keybindings: shortcuts::Overrides,
    hosts: Vec<HostDefinition>,
    #[serde(default)]
    forwards: BTreeMap<String, Vec<crate::ssh_forward::LocalForward>>,
    sessions: BTreeMap<String, Vec<crate::backend::SessionProfile>>,
    docker_endpoint: Option<String>,
    last_host: String,
    appearance: Appearance,
    sidebar_collapsed: bool,
    sidebar_width: Option<f32>,
    window_size: Option<[f32; 2]>,
    show_top_title: Option<bool>,
    show_status_bar: Option<bool>,
    local_name: String,
    metrics_config: metrics_config::Config,
    language: crate::i18n::Language,
}
impl Preferences {
    fn resolved_sidebar_opacity(&self) -> f32 {
        self.sidebar_opacity
            .filter(|v| v.is_finite())
            .unwrap_or(1.)
            .clamp(0.2, 1.)
    }

    fn path() -> PathBuf {
        std::env::var_os("TSHELL_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::config_dir()
                    .unwrap_or_else(|| PathBuf::from("data"))
                    .join("tshell")
            })
            .join("workspace.json")
    }
    fn load() -> Self {
        let Some(mut original) = std::fs::read(Self::path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        else {
            return Self::default();
        };
        let Some(preferences) = serde_json::from_value::<Self>(original.clone()).ok() else {
            return Self::default();
        };
        let migrated = preferences.migrated();
        let mut changed = false;
        if let Some(hosts) = original
            .get_mut("hosts")
            .and_then(|hosts| hosts.as_array_mut())
        {
            for (value, host) in hosts.iter_mut().zip(&migrated.hosts) {
                if value.get("id").and_then(|id| id.as_str()) != Some(host.id.to_string().as_str())
                {
                    if let Some(value) = value.as_object_mut() {
                        value.insert("id".into(), serde_json::Value::String(host.id.to_string()));
                        changed = true;
                    }
                }
            }
        }
        for (key, value) in [
            ("sessions", serde_json::to_value(&migrated.sessions)),
            ("forwards", serde_json::to_value(&migrated.forwards)),
        ] {
            if let Ok(value) = value
                && original.get(key).is_some_and(|old| old != &value)
            {
                original[key] = value;
                changed = true;
            }
        }
        if changed {
            if let Ok(bytes) = serde_json::to_vec_pretty(&original) {
                let path = Self::path();
                let temporary =
                    path.with_extension(format!("migration.{}.tmp", std::process::id()));
                if std::fs::write(&temporary, bytes).is_ok() {
                    if let Err(error) = std::fs::rename(&temporary, &path) {
                        tracing::warn!(%error,"host identity migration could not be saved");
                        let _ = std::fs::remove_file(temporary);
                    }
                }
            }
        }
        migrated
    }
    fn migrated(mut self) -> Self {
        if self.selected_theme.is_none() {
            self.selected_theme = self
                .legacy_custom_theme
                .clone()
                .or_else(|| self.legacy_terminal_theme.clone())
                .or_else(|| self.legacy_custom_theme_active.then(|| "custom-1".into()));
        }
        let mut identities = BTreeSet::new();
        let legacy_sessions_map = self.sessions.clone();
        let legacy_forwards_map = self.forwards.clone();
        let mut migrated_session_keys = BTreeSet::new();
        let mut migrated_forward_keys = BTreeSet::new();
        for host in &mut self.hosts {
            if !host.id.is_remote() || !identities.insert(host.id.clone()) {
                host.id = Default::default();
                identities.insert(host.id.clone());
            }
            let legacy_sessions = format!(
                "ssh:{}:{}:{}",
                host.ssh.port.unwrap_or(22),
                host.ssh.destination,
                host.ssh.user
            );
            let legacy_forwards = format!(
                "{}:{}:{}",
                host.ssh.destination,
                host.ssh.user,
                host.ssh.port.unwrap_or(22)
            );
            let key = host.id.to_string();
            if let Some(profiles) = legacy_sessions_map.get(&legacy_sessions).cloned() {
                self.sessions.entry(key.clone()).or_insert(profiles);
                migrated_session_keys.insert(legacy_sessions);
            }
            if let Some(rules) = legacy_forwards_map.get(&legacy_forwards).cloned() {
                self.forwards.entry(key).or_insert(rules);
                migrated_forward_keys.insert(legacy_forwards);
            }
        }
        for key in migrated_session_keys {
            self.sessions.remove(&key);
        }
        for key in migrated_forward_keys {
            self.forwards.remove(&key);
        }
        self
    }
    fn write_to(&self, path: &std::path::Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        if let Err(error) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        Ok(())
    }
}

pub(crate) fn saved_window_size() -> Option<Size<Pixels>> {
    valid_window_size(Preferences::load().window_size)
        .map(|[width, height]| size(px(width), px(height)))
}
fn valid_window_size(value: Option<[f32; 2]>) -> Option<[f32; 2]> {
    value.filter(|[width, height]| {
        width.is_finite()
            && height.is_finite()
            && (860. ..=8192.).contains(width)
            && (520. ..=8192.).contains(height)
    })
}
fn active_theme_id(
    themes: &ThemeFile,
    light_theme: &str,
    dark_theme: &str,
    appearance: Appearance,
    system: WindowAppearance,
) -> String {
    let light = appearance.mode(system) == ThemeMode::Light;
    let selected = if light { light_theme } else { dark_theme };
    themes
        .selected(selected)
        .map(|theme| theme.id.as_str())
        .unwrap_or_else(|| themes.fallback_id())
        .to_string()
}
fn theme_slots(themes: &ThemeFile, prefs: &Preferences) -> (String, String) {
    let legacy = prefs.selected_theme.as_deref();
    let slot = |configured: Option<&String>, light| {
        configured
            .map(String::as_str)
            .filter(|id| themes.selected(id).is_some())
            .or_else(|| {
                legacy
                    .and_then(|id| terminal_theme::paired_theme(id, light))
                    .filter(|id| themes.selected(id).is_some())
            })
            .or_else(|| {
                legacy
                    .and_then(|id| themes.selected(id))
                    .filter(|theme| theme.colors().light() == light)
                    .map(|theme| theme.id.as_str())
            })
            .or_else(|| {
                themes
                    .selected(if light {
                        "vscode-light"
                    } else {
                        terminal_theme::DEFAULT_THEME_ID
                    })
                    .map(|theme| theme.id.as_str())
            })
            .or_else(|| {
                themes
                    .themes
                    .iter()
                    .find(|theme| theme.colors().light() == light)
                    .map(|theme| theme.id.as_str())
            })
            .unwrap_or_else(|| themes.fallback_id())
            .to_string()
    };
    (
        slot(prefs.light_theme.as_ref(), true),
        slot(prefs.dark_theme.as_ref(), false),
    )
}
fn session_storage_key(config: Option<&HostDefinition>) -> Option<String> {
    match config {
        None => Some("local".into()),
        Some(host) if !host.tmux => Some(host.id.to_string()),
        Some(_) => None,
    }
}
fn forward_storage_key(config: &HostDefinition) -> String {
    config.id.to_string()
}

struct Host {
    pending: Vec<Action>,
    name: String,
    config: Option<HostDefinition>,
    backend: Option<Backend>,
    snapshot: Snapshot,
    views: BTreeMap<String, Entity<TerminalView>>,
    read_notices: BTreeMap<String, u64>,
    collapsed_sessions: BTreeSet<String>,
    viewport: Option<(String, PixelViewport)>,
}
impl Host {
    fn identity(&self) -> tshell_host::HostId {
        self.config
            .as_ref()
            .map(|host| host.id.clone())
            .unwrap_or_else(tshell_host::HostId::local)
    }

    fn connection(&self) -> Connection {
        if self.backend.is_some() {
            self.snapshot.connection
        } else {
            Connection::Closed
        }
    }

    fn can_disconnect(&self) -> bool {
        self.config.is_some() && self.backend.is_some()
    }

    fn disconnect(&mut self) -> bool {
        if !self.can_disconnect() {
            return false;
        }
        self.pending.clear();
        self.views.clear();
        self.read_notices.clear();
        self.collapsed_sessions.clear();
        self.viewport = None;
        self.backend = None;
        self.snapshot = Snapshot {
            connection: Connection::Closed,
            ..Default::default()
        };
        true
    }
}

#[cfg(test)]
mod host_tests {
    use super::{Action, Backend, Connection, Host, HostDefinition, LocalBackend, Snapshot};
    use std::collections::{BTreeMap, BTreeSet};

    fn host(config: Option<HostDefinition>) -> Host {
        Host {
            pending: vec![Action::NewSession],
            name: "Test host".into(),
            config,
            backend: Some(Backend::Local(
                LocalBackend::restore(std::env::temp_dir(), None, Some(Vec::new())).unwrap(),
            )),
            snapshot: Snapshot {
                connection: Connection::Ready,
                ..Default::default()
            },
            views: BTreeMap::new(),
            read_notices: BTreeMap::from([("pane".into(), 1)]),
            collapsed_sessions: BTreeSet::from(["session".into()]),
            viewport: None,
        }
    }

    #[test]
    fn disconnect_preserves_remote_configuration_and_clears_live_state() {
        let config = HostDefinition {
            id: Default::default(),
            ssh: tshell_host::SshConfig {
                destination: "example.invalid".into(),
                user: "test".into(),
                port: None,
                identity_file: None,
            },
            name: "Test host".into(),
            tmux: true,
            socket: None,
        };
        let mut host = host(Some(config.clone()));
        assert_eq!(host.connection(), Connection::Ready);
        assert!(host.disconnect());
        assert_eq!(host.config, Some(config));
        assert!(host.backend.is_none());
        assert_eq!(host.connection(), Connection::Closed);
        assert!(host.snapshot.sessions.is_empty());
        assert!(host.pending.is_empty());
        assert!(host.read_notices.is_empty());
        assert!(host.collapsed_sessions.is_empty());
        assert!(!host.can_disconnect());
        assert!(!host.disconnect());
    }

    #[test]
    fn local_host_cannot_be_disconnected() {
        let mut host = host(None);
        assert!(!host.can_disconnect());
        assert!(!host.disconnect());
        assert!(host.backend.is_some());
        assert_eq!(host.connection(), Connection::Ready);
        assert_eq!(host.pending.len(), 1);
    }

    #[test]
    fn host_without_a_backend_is_not_connecting_or_connected() {
        let mut host = host(None);
        host.backend = None;
        host.snapshot = Snapshot::default();
        assert_eq!(host.connection(), Connection::Closed);
    }
}
type WorkspaceKey = (tshell_host::HostId, String);
struct SessionWorkspace {
    cwd: Option<PathBuf>,
    page: Page,
    explorer: Option<Entity<Explorer>>,
    git: Option<Entity<GitView>>,
    refresh: bool,
    subscriptions: Vec<Subscription>,
}
impl Default for SessionWorkspace {
    fn default() -> Self {
        Self {
            cwd: None,
            page: Page::Terminal,
            explorer: None,
            git: None,
            refresh: false,
            subscriptions: Vec::new(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Terminal,
    Explorer,
    Git,
    Docker,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ConnectionLatency {
    ssh: Option<Duration>,
    tmux: Option<Duration>,
}

#[derive(Clone, Copy)]
enum ModalKind {
    Settings,
    Host,
    NewTab,
    Commands,
    ThemeEditor,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostField {
    Name,
    Destination,
    User,
    Port,
    IdentityFile,
}

fn command_icon(action: Shortcut) -> IconName {
    match action {
        Shortcut::ShowFiles | Shortcut::RefreshFiles => IconName::FolderOpen,
        Shortcut::ShowGit | Shortcut::RefreshGit => IconName::GitBranch,
        Shortcut::ShowTerminal | Shortcut::NewWindow => IconName::Terminal,
        Shortcut::EditHost => IconName::Pencil,
        Shortcut::CloseWindow | Shortcut::ClosePane | Shortcut::RemoveHost => IconName::X,
        Shortcut::Session(_) | Shortcut::SelectSession(_) => IconName::Layers,
        Shortcut::AddHost => IconName::ServerPlus,
        Shortcut::PreviousHost => IconName::ChevronLeft,
        Shortcut::NextHost => IconName::ChevronRight,
        Shortcut::NewColumn => IconName::Columns2,
        Shortcut::NewRow => IconName::Rows2,
        Shortcut::Reconnect => IconName::RefreshCw,
        Shortcut::SaveFile => IconName::Save,
        Shortcut::Settings => IconName::Settings,
        Shortcut::ToggleSidebar => IconName::PanelLeft,
        Shortcut::Zen | Shortcut::Focus => IconName::Focus,
        Shortcut::CycleTheme => IconName::SunMoon,
        Shortcut::Copy => IconName::Copy,
        Shortcut::TerminalSearch => IconName::Search,
        Shortcut::Paste => IconName::ClipboardPaste,
        Shortcut::CommandPalette => IconName::Command,
        _ => IconName::ArrowRight,
    }
}

pub struct AppView {
    #[cfg(windows)]
    updater: crate::update::Updater,
    line_height_scale: crate::terminal_metrics::LineHeight,
    workspace_updates: async_channel::Sender<()>,
    metrics_target: Option<host_metrics::Target>,
    metrics_selection: BTreeSet<metrics_config::Metric>,
    metrics_config: metrics_config::Config,
    metrics_monitor: Option<host_metrics::Monitor>,
    metrics_epoch: u64,
    metrics: Option<Result<host_metrics::Stats, String>>,
    connection_latency: ConnectionLatency,
    latency_task: Option<Task<()>>,
    ligatures: bool,
    light_theme: String,
    dark_theme: String,
    themes: ThemeFile,
    theme_error: Option<String>,
    terminal_palette: Palette,
    hosts: Vec<Host>,
    session_profiles: BTreeMap<String, Vec<crate::backend::SessionProfile>>,
    active: usize,
    font_size: f32,
    window_size: Option<[f32; 2]>,
    show_top_title: bool,
    show_status_bar: bool,
    font_family: String,
    background_opacity: f32,
    sidebar_opacity: f32,
    acrylic_background: bool,
    keybindings: shortcuts::Overrides,
    settings_ui: SettingsUi,
    shortcut_map: shortcuts::Keymap,
    appearance: Appearance,
    palette: Palette,
    language: crate::i18n::Language,
    use_tmux: bool,
    settings: bool,
    command_palette: bool,
    page: Page,
    sidebar_collapsed: bool,
    sidebar_width: f32,
    sidebar_drag: Option<(Pixels, f32)>,
    zen: bool,
    floating: bool,
    float_position: Point<Pixels>,
    float_drag: Option<Point<Pixels>>,
    editing_host: Option<usize>,
    host_error_field: Option<HostField>,
    creating_tab: bool,
    editing_session: Option<String>,
    home_task: Option<Task<()>>,

    label: Entity<InputState>,
    destination: Entity<InputState>,
    user: Entity<InputState>,
    port: Entity<InputState>,
    identity_file: Entity<InputState>,
    tab_path: Entity<InputState>,
    tab_name: Entity<InputState>,
    directory_candidates: Vec<String>,
    directory_task: Option<Task<()>>,
    directory_error: Option<String>,
    validated_directory: Option<(usize, String, String)>,
    command_state: Entity<CommandState>,
    forward_local_port: Entity<InputState>,
    forward_target_port: Entity<InputState>,
    forward_focus: FocusHandle,
    forward_kind: crate::ssh_forward::ForwardKind,
    forward_form: bool,
    forward_add_form: bool,
    // The dialog builder runs inside AppView's render and cannot borrow its owner again.
    forward_dialog_height: std::rc::Rc<std::cell::Cell<f32>>,
    forward_host_index: Option<usize>,
    forward_error: Option<String>,
    forwards: BTreeMap<String, Vec<crate::ssh_forward::LocalForward>>,
    forward_processes: BTreeMap<(String, usize), crate::ssh_forward::Tunnel>,
    docker: Entity<tshell_docker::DockerView>,
    docker_endpoint: Option<String>,
    workspaces: BTreeMap<WorkspaceKey, SessionWorkspace>,
    active_workspace: Option<WorkspaceKey>,
    transfer_queue: transfer_queue::TransferQueue,
    tool_request: u64,
    navigation_request: u64,
    navigation_loading: Option<String>,
    message: Option<String>,
    root_focus: FocusHandle,
    keyboard_focus: bool,
    _focus_subscription: Subscription,
    cell_width: f32,
    line_height: f32,
    need_focus: bool,
    default_cwd: PathBuf,
    cwd: PathBuf,
}

struct ModalContent {
    owner: WeakEntity<AppView>,
    kind: ModalKind,
    _subscription: Subscription,
}
impl Render for ModalContent {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.owner
            .update(cx, |app, cx| match self.kind {
                ModalKind::Settings => app.settings_view(_window, cx),
                ModalKind::Host => app.connection_editor(cx),
                ModalKind::NewTab => app.new_session_editor(cx),
                ModalKind::Commands => app.command_view(_window, cx),
                ModalKind::ThemeEditor => app.theme_editor_view(cx),
                ModalKind::Forward => app.forward_editor(cx),
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}

use tshell_ui::controls::{button, icon_button};

const PANE_STROKE: f32 = 1.;

// Shared divider/frame bounds include the stroke centered in each gap.
fn divider_span(start: f32, extent: f32, total: f32, gap: f32) -> (f32, f32) {
    let from = (start - (gap + PANE_STROKE) / 2.).max(0.);
    let to = (start + extent + (gap + PANE_STROKE) / 2.).min(total);
    (from - start, (to - from).max(0.))
}

fn active_dividers(
    pane: &PaneBounds,
    width: f32,
    height: f32,
    gap_x: f32,
    gap_y: f32,
) -> Vec<Bounds<Pixels>> {
    let (dx, w) = divider_span(pane.x, pane.width, width, gap_x);
    let (dy, h) = divider_span(pane.y, pane.height, height, gap_y);
    let (x, y) = (pane.x + dx, pane.y + dy);
    [
        (pane.x > 0.5, x, y, PANE_STROKE, h),
        (
            pane.x + pane.width < width - 0.5,
            x + w - PANE_STROKE,
            y,
            PANE_STROKE,
            h,
        ),
        (pane.y > 0.5, x, y, w, PANE_STROKE),
        (
            pane.y + pane.height < height - 0.5,
            x,
            y + h - PANE_STROKE,
            w,
            PANE_STROKE,
        ),
    ]
    .into_iter()
    .filter(|(internal, ..)| *internal)
    .map(|(_, x, y, w, h)| Bounds::new(point(px(x), px(y)), size(px(w), px(h))))
    .collect()
}

fn leading_inset(extent: f32, cells: usize, cell_size: f32, minimum: f32) -> f32 {
    (extent - cells as f32 * cell_size).max(0.).min(minimum)
}

fn chrome_line_height(window: &Window, size: f32) -> Pixels {
    let text = window.text_system();
    let face = text.resolve_font(&font("Segoe UI"));
    let ascent = f32::from(text.ascent(face, px(size)));
    let descent = f32::from(text.descent(face, px(size))).abs();
    px((ascent + descent).ceil())
}

fn window_controls(window: &Window, foreground: u32) -> impl IntoElement {
    let maximize_icon = if window.is_maximized() {
        IconName::WindowRestore
    } else {
        IconName::WindowMaximize
    };
    div().h_full().flex().children(
        [
            (
                "window-minimize",
                IconName::WindowMinimize,
                WindowControlArea::Min,
            ),
            ("window-maximize", maximize_icon, WindowControlArea::Max),
            (
                "window-close",
                IconName::WindowClose,
                WindowControlArea::Close,
            ),
        ]
        .into_iter()
        .map(move |(id, icon, area)| {
            div()
                .id(id)
                .w(px(WINDOW_CONTROL_WIDTH))
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |style| style.bg(rgba((foreground << 8) | 0x14)))
                .when(cfg!(windows), |view| view.window_control_area(area))
                .when(!cfg!(windows), |view| {
                    view.on_click(move |_, window, _| match area {
                        WindowControlArea::Min => window.minimize_window(),
                        WindowControlArea::Max => window.zoom_window(),
                        WindowControlArea::Close => window.remove_window(),
                        WindowControlArea::Drag => {}
                    })
                })
                .child(icon)
        }),
    )
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.set_reduce_motion(true);
        cx.bind_keys([KeyBinding::new(
            "tab",
            NoAction,
            Some("DirectoryCompletion"),
        )]);
        let prefs = Preferences::load();
        // Every translated string built below needs the locale in place first.
        crate::i18n::set_language(prefs.language);
        let theme_result = ThemeFile::ensure_file(&Self::theme_path(), &Self::legacy_theme_path());
        let theme_error = theme_result.as_ref().err().map(ToString::to_string);
        let themes = theme_result.unwrap_or_default();
        let (light_theme, dark_theme) = theme_slots(&themes, &prefs);
        let active_theme = active_theme_id(
            &themes,
            &light_theme,
            &dark_theme,
            prefs.appearance,
            window.appearance(),
        );
        let selected_theme = themes
            .selected(&active_theme)
            .expect("selected theme must exist");
        let terminal_palette = selected_theme.palette();
        let palette = selected_theme.ui_palette();
        crate::terminal_protocol::set_default_theme(terminal_palette.terminal_theme());
        let fonts = window.text_system().all_font_names();
        let font_family = if fonts.contains(&prefs.font_family) {
            prefs.font_family.clone()
        } else {
            "Consolas".to_string()
        };
        let background_opacity = prefs
            .background_opacity
            .filter(|v| v.is_finite())
            .unwrap_or(1.)
            .clamp(0.2, 1.);
        let sidebar_opacity = prefs.resolved_sidebar_opacity();
        let cwd = std::env::current_dir().unwrap_or_default();
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let (backend, message) =
            match LocalBackend::restore(cwd.clone(), None, prefs.sessions.get("local").cloned()) {
                Ok(b) => (Some(Backend::Local(b)), None),
                Err(e) => (
                    None,
                    Some(
                        crate::t!("ws.start_failed", error = crate::i18n::format_error(&e))
                            .to_string(),
                    ),
                ),
            };
        let mut hosts = vec![Host {
            name: if matches!(prefs.local_name.as_str(), "" | "本机") {
                "local".into()
            } else {
                prefs.local_name
            },
            config: None,
            backend,
            snapshot: Snapshot::default(),
            views: BTreeMap::new(),
            read_notices: BTreeMap::new(),
            collapsed_sessions: BTreeSet::new(),
            viewport: None,

            pending: Vec::new(),
        }];
        for config in prefs.hosts {
            hosts.push(Host {
                name: if config.name.is_empty() {
                    config.ssh.destination.clone()
                } else {
                    config.name.clone()
                },
                config: Some(config),
                backend: None,
                snapshot: Snapshot::default(),
                views: BTreeMap::new(),
                read_notices: BTreeMap::new(),
                collapsed_sessions: BTreeSet::new(),
                viewport: None,

                pending: Vec::new(),
            });
        }
        let docker_targets = docker::targets(&hosts, prefs.docker_endpoint.clone());
        let docker = cx.new(|cx| {
            tshell_docker::DockerView::new(
                docker_targets,
                palette,
                terminal_palette,
                font_family.clone(),
                if prefs.font_size >= 10. {
                    prefs.font_size.min(24.)
                } else {
                    14.
                },
                window,
                cx,
            )
        });
        let (workspace_updates, updates) = async_channel::bounded(1);
        let initial_window_size =
            valid_window_size(prefs.window_size).unwrap_or(DEFAULT_WINDOW_SIZE);
        #[cfg(windows)]
        let (updater, update_events) = crate::update::Updater::start();
        let settings_ui = SettingsUi::new(
            fonts,
            &font_family,
            background_opacity,
            sidebar_opacity,
            initial_window_size,
            &themes,
            &light_theme,
            &dark_theme,
            window,
            cx,
        );
        let owner = cx.entity().downgrade();
        let focus_window = window.window_handle();
        // Native Tab actions can consume the key before view-level handlers run.
        let focus_subscription = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != focus_window
                || event.keystroke.key != "tab"
                || event.keystroke.modifiers.control
                || event.keystroke.modifiers.alt
            {
                return;
            }
            let _ = owner.update(cx, |app, cx| {
                if app.settings_ui.recording.is_none()
                    && (window.has_active_dialog(cx) || app.page != Page::Terminal)
                {
                    app.set_keyboard_focus(true, cx);
                }
            });
        });
        let mut this = Self {
            #[cfg(windows)]
            updater,
            metrics_target: None,
            metrics_selection: BTreeSet::new(),
            metrics_config: prefs.metrics_config.normalized(),
            metrics_monitor: None,
            metrics_epoch: 0,
            metrics: None,
            connection_latency: ConnectionLatency::default(),
            latency_task: None,
            workspace_updates,
            ligatures: prefs.ligatures,
            line_height_scale: prefs.line_height_scale,
            light_theme,
            dark_theme,
            themes,
            theme_error,
            terminal_palette,
            hosts,
            session_profiles: prefs.sessions,
            active: 0,
            settings_ui,
            font_family,
            background_opacity,
            sidebar_opacity,
            acrylic_background: prefs.acrylic_background,
            shortcut_map: shortcuts::Keymap::new(&prefs.keybindings),
            keybindings: prefs.keybindings,
            font_size: if prefs.font_size >= 10. {
                prefs.font_size.min(24.)
            } else {
                14.
            },
            window_size: Some(initial_window_size),
            show_top_title: prefs.show_top_title.unwrap_or(true),
            show_status_bar: prefs.show_status_bar.unwrap_or(true),
            appearance: prefs.appearance,
            palette,
            language: prefs.language,
            use_tmux: true,
            settings: false,
            command_palette: false,
            page: Page::Terminal,
            sidebar_collapsed: prefs.sidebar_collapsed,
            sidebar_width: prefs
                .sidebar_width
                .filter(|width| width.is_finite())
                .unwrap_or(SIDEBAR_WIDTH)
                .clamp(160., 520.),
            sidebar_drag: None,
            zen: false,
            floating: false,
            float_position: point(px(24.), px(24.)),
            float_drag: None,
            editing_host: None,
            host_error_field: None,
            creating_tab: false,
            editing_session: None,
            home_task: None,

            label: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::t!("ws.placeholder_name"))
            }),
            destination: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(crate::t!("ws.placeholder_destination"))
                    .default_value(prefs.last_host)
            }),
            user: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::t!("ws.placeholder_user"))
            }),
            port: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::t!("ws.placeholder_port"))
            }),
            identity_file: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::t!("ws.placeholder_identity_file"))
            }),
            directory_candidates: Vec::new(),
            directory_task: None,
            directory_error: None,
            validated_directory: None,
            tab_name: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::t!("ws.placeholder_tab_name"))
            }),
            tab_path: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::t!("ws.placeholder_tab_path"))
            }),
            command_state,
            forward_local_port: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::t!("ssh.forward_local_port"))
            }),
            forward_target_port: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::t!("ssh.forward_target_port"))
            }),
            forward_kind: crate::ssh_forward::ForwardKind::Local,
            forward_form: false,
            forward_add_form: false,
            forward_focus: cx.focus_handle(),
            forward_dialog_height: std::rc::Rc::new(std::cell::Cell::new(180.)),
            forward_host_index: None,
            forward_error: None,
            forwards: prefs.forwards,
            forward_processes: BTreeMap::new(),
            docker,
            docker_endpoint: prefs.docker_endpoint.clone(),
            workspaces: BTreeMap::new(),
            active_workspace: None,
            transfer_queue: Default::default(),
            tool_request: 0,
            navigation_request: 0,
            navigation_loading: None,
            message,
            root_focus: cx.focus_handle(),
            keyboard_focus: false,
            _focus_subscription: focus_subscription,
            cell_width: 8.4,
            line_height: 21.,
            need_focus: true,
            default_cwd: cwd.clone(),
            cwd,
        };
        for input in [
            &this.destination,
            &this.user,
            &this.port,
            &this.identity_file,
            &this.label,
        ] {
            cx.subscribe_in(input, window, |this, _, event, window, cx| {
                if this.settings_ui.host_form && matches!(event, InputEvent::PressEnter { .. }) {
                    this.add_host(window, cx);
                }
            })
            .detach();
        }
        cx.subscribe_in(
            &this.tab_path,
            window,
            |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.create_tab(window, cx),
                InputEvent::Change if this.creating_tab => this.complete_tab_directory(cx),
                _ => {}
            },
        )
        .detach();
        cx.subscribe_in(&this.tab_name, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) && this.creating_tab {
                this.create_tab(window, cx);
            }
        })
        .detach();
        cx.subscribe_in(&this.docker, window, |this, _, event, window, cx| {
            this.docker_event(event.clone(), window, cx)
        })
        .detach();
        cx.observe(&this.docker, |_, _, cx| cx.notify()).detach();
        this.apply_appearance(window, cx);
        this.subscribe_settings(window, cx);
        this.apply_opacity(window, cx);
        cx.observe_window_appearance(window, |this, window, cx| {
            this.apply_appearance(window, cx);
        })
        .detach();
        this.sync(cx);
        this.sync_latency_monitor(window, cx);
        let activations = crate::terminal_notifications::subscribe_activations();
        cx.spawn_in(window, async move |view, cx| {
            while let Ok(identity) = activations.recv().await {
                if view
                    .update_in(cx, |this, window, cx| {
                        this.focus_terminal_notice(identity, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let auth_requests = crate::ssh_pool::interactive::subscribe();
        cx.spawn_in(window, async move |view, cx| {
            while let Ok(prompt) = auth_requests.recv().await {
                let finished = prompt.finished.clone();
                if view
                    .update_in(cx, |this, window, cx| {
                        this.open_auth_prompt(prompt, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
                let _ = finished.recv().await;
            }
        })
        .detach();
        cx.spawn(async move |view, cx| {
            while updates.recv().await.is_ok() {
                cx.background_executor()
                    .timer(Duration::from_millis(4))
                    .await;
                while updates.try_recv().is_ok() {}
                if view.update(cx, |this, cx| this.sync(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        #[cfg(windows)]
        this.listen_for_updates(update_events, window, cx);
        let configs: Vec<_> = this
            .hosts
            .iter()
            .filter_map(|host| host.config.clone())
            .collect();
        for config in configs {
            this.start_forwards_for(&config);
        }
        this
    }
    fn apply_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_theme = active_theme_id(
            &self.themes,
            &self.light_theme,
            &self.dark_theme,
            self.appearance,
            window.appearance(),
        );
        let selected_theme = self
            .themes
            .selected(&active_theme)
            .unwrap_or_else(|| self.themes.selected(self.themes.fallback_id()).unwrap());
        self.terminal_palette = selected_theme.palette();
        let terminal_theme = self.terminal_palette.terminal_theme();
        self.palette = selected_theme.ui_palette();
        crate::terminal_protocol::set_default_theme(terminal_theme);
        let mode = if terminal_theme.light() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(mode, Some(window), cx);
        let theme = Theme::global_mut(cx);
        self.palette.apply_ui(theme);
        theme.sheet.margin_top = px(0.);
        Theme::sync_base(cx);
        for h in &self.hosts {
            if let Some(backend) = &h.backend {
                backend.set_theme(terminal_theme);
            }
            for v in h.views.values() {
                v.update(cx, |v, cx| {
                    v.palette = self.terminal_palette;
                    cx.notify();
                });
            }
        }
        cx.notify();
    }
    fn save(&mut self, cx: &App) {
        if let Err(e) = self.save_preferences(cx) {
            self.message = Some(crate::t!("ws.save_failed", error = e.to_string()).to_string());
        }
    }
    fn save_preferences(&self, cx: &App) -> anyhow::Result<()> {
        let prefs = Preferences {
            line_height_scale: self.line_height_scale,
            ligatures: self.ligatures,
            selected_theme: None,
            light_theme: Some(self.light_theme.clone()),
            dark_theme: Some(self.dark_theme.clone()),
            legacy_terminal_theme: None,
            legacy_custom_theme: None,
            legacy_custom_theme_active: false,
            font_size: self.font_size,
            font_family: self.font_family.clone(),
            background_opacity: Some(self.background_opacity),
            sidebar_opacity: Some(self.sidebar_opacity),
            acrylic_background: self.acrylic_background,
            keybindings: self.keybindings.clone(),
            forwards: self.forwards.clone(),
            docker_endpoint: self.docker_endpoint.clone(),
            hosts: self.hosts.iter().filter_map(|h| h.config.clone()).collect(),
            sessions: self
                .hosts
                .iter()
                .filter_map(|host| session_storage_key(host.config.as_ref()))
                .filter_map(|key| {
                    self.session_profiles
                        .get(&key)
                        .map(|profiles| (key, profiles.clone()))
                })
                .collect(),
            last_host: self.destination.read(cx).value().to_string(),
            appearance: self.appearance,
            sidebar_collapsed: self.sidebar_collapsed,
            sidebar_width: Some(self.sidebar_width),
            window_size: self.window_size,
            show_top_title: Some(self.show_top_title),
            show_status_bar: Some(self.show_status_bar),
            local_name: self.hosts[0].name.clone(),
            metrics_config: self.metrics_config.clone(),
            language: self.language,
        };
        prefs.write_to(&Preferences::path())
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let mut profiles_changed = false;
        for (index, host) in self.hosts.iter_mut().enumerate() {
            if let Some(backend) = &mut host.backend {
                backend.listen(self.workspace_updates.clone());
                backend.reap_finished();
                if let Backend::Local(local) = backend {
                    if let Some(key) = session_storage_key(host.config.as_ref()) {
                        let profiles = local.profiles();
                        if self.session_profiles.get(&key) != Some(&profiles) {
                            self.session_profiles.insert(key, profiles);
                            profiles_changed = true;
                        }
                    }
                }
                let snapshot = backend.snapshot();
                if snapshot.revision != host.snapshot.revision
                    || snapshot.connection != host.snapshot.connection
                {
                    let active_key = |s: &Snapshot| {
                        s.window().map(|w| {
                            (
                                s.active_session.clone(),
                                w.id.clone(),
                                w.active_pane.clone(),
                            )
                        })
                    };
                    if index == self.active && active_key(&snapshot) != active_key(&host.snapshot) {
                        self.need_focus = true;
                    }
                    host.snapshot = snapshot;
                    if host.snapshot.connected() {
                        host.collapsed_sessions
                            .retain(|id| host.snapshot.sessions.iter().any(|s| &s.id == id));
                    }
                    cx.notify();
                }
                if host.snapshot.connected() && !host.pending.is_empty() {
                    for action in std::mem::take(&mut host.pending) {
                        if let Err(e) = backend.apply(action) {
                            self.message = Some(e.to_string());
                        }
                    }
                }
                let ids: BTreeSet<_> = host
                    .snapshot
                    .windows()
                    .flat_map(|w| &w.panes)
                    .map(|p| p.id.clone())
                    .collect();
                host.views.retain(|id, _| ids.contains(id));
                host.read_notices.retain(|id, _| ids.contains(id));
                if index == self.active {
                    if let Some(w) = host.snapshot.window() {
                        for pane in &w.panes {
                            if !host.views.contains_key(&pane.id) {
                                if let Some(screen) = backend.screen(&pane.id) {
                                    let font = self.font_size;
                                    let palette = self.terminal_palette;
                                    host.views.insert(
                                        pane.id.clone(),
                                        cx.new(|cx| {
                                            let mut view = TerminalView::from_session(
                                                screen, font, palette, cx,
                                            );
                                            view.font_family = self.font_family.clone();
                                            view.line_height_scale = self.line_height_scale;
                                            view.ligatures = self.ligatures;
                                            view.background_inherited = true;
                                            view
                                        }),
                                    );
                                    self.need_focus = true;
                                    cx.notify();
                                }
                            }
                        }
                    }
                }
            }
        }
        if profiles_changed {
            self.save(cx);
        }
        self.workspaces.retain(|(id, session), _| {
            self.hosts
                .iter()
                .find(|host| host.identity() == *id)
                .is_some_and(|host| {
                    host.backend.is_none()
                        || !host.snapshot.connected()
                        || host
                            .snapshot
                            .sessions
                            .iter()
                            .any(|live| live.id == *session)
                })
        });
        let host = &self.hosts[self.active];
        let key = host
            .snapshot
            .session()
            .map(|session| (host.identity(), session.id.clone()));
        if self.active_workspace != key {
            if let Some(old) = self
                .active_workspace
                .as_ref()
                .and_then(|key| self.workspaces.get_mut(key))
                && self.page != Page::Docker
            {
                old.page = self.page;
            }
            if self.page != Page::Docker {
                self.page = key
                    .as_ref()
                    .and_then(|key| self.workspaces.get(key))
                    .map(|workspace| workspace.page)
                    .unwrap_or(Page::Terminal);
            }
            self.active_workspace = key.clone();
            self.need_focus = true;
        }
        if let Some(key) = key {
            self.workspaces.entry(key).or_default();
        }
        self.cwd = self
            .active_workspace
            .as_ref()
            .and_then(|key| self.workspaces.get(key))
            .and_then(|workspace| workspace.cwd.clone())
            .unwrap_or_else(|| {
                if host.config.is_some() {
                    PathBuf::from("~")
                } else {
                    dirs::home_dir().unwrap_or_else(|| self.default_cwd.clone())
                }
            });
        self.sync_metrics(cx);
    }

    fn sync_latency_monitor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (ssh_enabled, tmux_enabled) = self.metrics_config.probes();
        if !ssh_enabled {
            self.connection_latency.ssh = None;
        }
        if !tmux_enabled {
            self.connection_latency.tmux = None;
        }
        if (!ssh_enabled && !tmux_enabled) || !self.show_status_bar || self.zen {
            self.latency_task = None;
            self.connection_latency = ConnectionLatency::default();
        } else if self.latency_task.is_none() {
            self.start_latency_monitor(window, cx);
        }
    }

    fn start_latency_monitor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.latency_task = Some(cx.spawn_in(window, async move |view, cx| {
            loop {
                let target = match view.update(cx, |this, _| {
                    if !this.show_status_bar || this.zen || this.page != Page::Terminal {
                        return None;
                    }
                    let probes = this.metrics_config.probes();
                    if probes == (false, false) {
                        return None;
                    }
                    let host = &this.hosts[this.active];
                    if host.backend.is_none() || !host.snapshot.connected() {
                        return None;
                    }
                    host.config.clone().map(|config| {
                        let tmux = host.backend.as_ref().and_then(|backend| match backend {
                            Backend::Tmux(client) => Some(client.clone()),
                            Backend::Local(_) => None,
                        });
                        (this.active, config, tmux, probes)
                    })
                }) {
                    Ok(Some(target)) => target,
                    Ok(None) => {
                        if view
                            .update(cx, |this, cx| {
                                if this.connection_latency != ConnectionLatency::default() {
                                    this.connection_latency = ConnectionLatency::default();
                                    cx.notify();
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                        cx.background_executor().timer(Duration::from_secs(2)).await;
                        continue;
                    }
                    Err(_) => break,
                };
                let (index, config, tmux, (ssh_enabled, tmux_enabled)) = target;
                let ssh = if ssh_enabled {
                    crate::ssh_pool::ping_async(config.ssh.clone())
                        .recv()
                        .await
                        .ok()
                        .and_then(Result::ok)
                } else {
                    None
                };
                let tmux_latency = if tmux_enabled {
                    match tmux.and_then(|client| client.probe().ok()) {
                        Some(receiver) => receiver.recv().await.ok().and_then(Result::ok),
                        None => None,
                    }
                } else {
                    None
                };
                if view
                    .update(cx, |this, cx| {
                        if this.show_status_bar
                            && !this.zen
                            && this.page == Page::Terminal
                            && this.active == index
                            && this.hosts[index].config.as_ref() == Some(&config)
                        {
                            let (show_ssh, show_tmux) = this.metrics_config.probes();
                            this.connection_latency = ConnectionLatency {
                                ssh: if show_ssh { ssh } else { None },
                                tmux: if show_tmux { tmux_latency } else { None },
                            };
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        }));
    }

    fn act(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.closes_session_with_unsaved_files(&action, cx) {
            self.message = Some(crate::t!("ws.session_files_busy").to_string());
            cx.notify();
            return;
        }
        self.message = None;
        if let Some(b) = &mut self.hosts[self.active].backend {
            if let Err(e) = b.apply(action) {
                self.message = Some(crate::i18n::format_error(&e));
            }
        }
        self.sync(cx);
        cx.notify();
    }
    fn switch_host(&mut self, index: usize, cx: &mut Context<Self>) {
        self.active = index;
        self.connection_latency = ConnectionLatency::default();

        self.message = None;
        self.need_focus = true;
        let host = &mut self.hosts[index];
        if host.backend.is_none() && self.page != Page::Docker {
            if let Some(config) = host.config.clone() {
                host.backend = if config.tmux {
                    Some(Backend::Tmux(TmuxClient::connect(config)))
                } else {
                    match LocalBackend::restore(
                        self.default_cwd.clone(),
                        Some(config.ssh.clone()),
                        session_storage_key(Some(&config))
                            .and_then(|key| self.session_profiles.get(&key).cloned()),
                    ) {
                        Ok(b) => Some(Backend::Local(b)),
                        Err(e) => {
                            self.message = Some(e.to_string());
                            None
                        }
                    }
                };
                host.snapshot = Snapshot::default();
            }
        }
        self.sync(cx);
        cx.notify();
    }
    fn reconnect(&mut self, cx: &mut Context<Self>) {
        let h = &mut self.hosts[self.active];
        h.views.clear();
        h.read_notices.clear();
        h.backend = None;
        h.viewport = None;
        h.snapshot = Snapshot::default();
        if h.config.is_none() {
            match LocalBackend::restore(
                self.default_cwd.clone(),
                None,
                self.session_profiles.get("local").cloned(),
            ) {
                Ok(b) => h.backend = Some(Backend::Local(b)),
                Err(e) => self.message = Some(e.to_string()),
            }
        }
        self.switch_host(self.active, cx);
    }
    fn disconnect_host(&mut self, index: usize, cx: &mut Context<Self>) {
        if self
            .hosts
            .get(index)
            .is_none_or(|host| host.config.is_none() || host.backend.is_none())
        {
            return;
        }
        let Some(host) = self.hosts.get_mut(index) else {
            return;
        };
        if !host.disconnect() {
            return;
        }
        if index == self.active {
            self.message = None;
            self.latency_task = None;
            self.connection_latency = ConnectionLatency::default();
        }
        self.sync(cx);
        cx.notify();
    }
    fn add_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.host_error_field = None;
        let name = self.label.read(cx).value().trim().to_owned();
        if name.chars().any(char::is_control) || name.chars().count() > 80 {
            self.host_validation_error(
                HostField::Name,
                crate::t!("ws.name_too_long").into_owned(),
                window,
                cx,
            );
            return;
        }
        if self.editing_host == Some(0) {
            if name.is_empty() {
                self.host_validation_error(
                    HostField::Name,
                    crate::t!("ws.name_required").into_owned(),
                    window,
                    cx,
                );
                return;
            }
            self.hosts[0].name = name;
            self.finish_host_editor(window, cx);
            return;
        }
        let destination = match validate_destination(&self.destination.read(cx).value()) {
            Ok(v) => v,
            Err(e) => {
                self.host_validation_error(HostField::Destination, e.to_string(), window, cx);
                return;
            }
        };
        if destination.contains('@') {
            self.host_validation_error(
                HostField::Destination,
                crate::t!("ws.host_without_user").into_owned(),
                window,
                cx,
            );
            return;
        }
        let user = self.user.read(cx).value().trim().to_owned();
        if user.is_empty()
            || user.chars().any(|c| c.is_whitespace() || c.is_control())
            || user.starts_with('-')
        {
            self.host_validation_error(
                HostField::User,
                crate::t!("ws.user_invalid").into_owned(),
                window,
                cx,
            );
            return;
        }
        let identity_file = self.identity_file.read(cx).value().trim().to_owned();
        let identity_file = if identity_file.is_empty() {
            None
        } else {
            let path = PathBuf::from(&identity_file);
            if !path.is_file() {
                self.host_validation_error(
                    HostField::IdentityFile,
                    crate::t!("ws.identity_file_invalid").into_owned(),
                    window,
                    cx,
                );
                return;
            }
            Some(path)
        };
        let text = self.port.read(cx).value();
        let port = if text.trim().is_empty() {
            None
        } else {
            match text.trim().parse::<u16>() {
                Ok(p) if p > 0 => Some(p),
                _ => {
                    self.host_validation_error(
                        HostField::Port,
                        crate::t!("ws.port_invalid").into_owned(),
                        window,
                        cx,
                    );
                    return;
                }
            }
        };
        let config = HostDefinition {
            id: self
                .editing_host
                .and_then(|index| self.hosts.get(index))
                .and_then(|host| host.config.as_ref())
                .map(|host| host.id.clone())
                .unwrap_or_default(),
            ssh: tshell_host::SshConfig {
                destination: destination.clone(),
                user,
                port,
                identity_file,
            },
            name: if name.is_empty() {
                destination.clone()
            } else {
                name.clone()
            },
            tmux: self.use_tmux,
            socket: None,
        };
        if let Some(index) = self.editing_host {
            let old = self.hosts[index].config.as_ref().unwrap().clone();
            let changed = old.ssh.destination != config.ssh.destination
                || old.ssh.user != config.ssh.user
                || old.ssh.port != config.ssh.port
                || old.ssh.identity_file != config.ssh.identity_file
                || old.tmux != config.tmux;
            if changed && !self.prepare_host_connection_change(index, cx) {
                return;
            }
            if changed {
                self.stop_forwards_for(&old);
                let old_key = forward_storage_key(&old);
                let new_key = forward_storage_key(&config);
                if old_key != new_key {
                    if let Some(rules) = self.forwards.remove(&old_key) {
                        self.forwards.entry(new_key).or_default().extend(rules);
                    }
                }
            }
            self.hosts[index].name = config.name.clone();
            self.hosts[index].config = Some(config.clone());
            if changed {
                self.hosts[index].backend = None;
                self.hosts[index].views.clear();
                self.hosts[index].viewport = None;
                self.start_forwards_for(&config);
            }
        } else if !self.hosts.iter().any(|h| {
            h.config.as_ref().is_some_and(|host| {
                host.ssh == config.ssh && host.tmux == config.tmux && host.name == config.name
            })
        }) {
            self.hosts.push(Host {
                name: config.name.clone(),
                config: Some(config),
                backend: None,
                snapshot: Snapshot::default(),
                views: BTreeMap::new(),
                read_notices: BTreeMap::new(),
                collapsed_sessions: BTreeSet::new(),
                viewport: None,

                pending: Vec::new(),
            });
        }
        self.finish_host_editor(window, cx);
    }
    fn finish_host_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_keys(cx);
        self.save(cx);
        if self.hosts[self.active].backend.is_none() {
            self.switch_host(self.active, cx);
        }
        self.cancel_host_editor(window, cx);
    }
    fn cancel_host_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        self.host_editor_closed(cx);
        if self.settings {
            window.focus(&self.settings_ui.focus, cx);
        }
    }
    fn host_editor_closed(&mut self, cx: &mut Context<Self>) {
        self.settings_ui.host_form = false;
        self.editing_host = None;
        self.host_error_field = None;
        self.message = None;
        self.need_focus = !self.settings;
        cx.notify();
    }
    fn modal_closed(&mut self, cx: &mut Context<Self>) {
        if self.settings {
            self.save(cx);
        }
        self.settings_ui.recording = None;
        self.settings_ui.host_form = false;
        self.forward_form = false;
        self.forward_add_form = false;
        self.forward_host_index = None;
        self.forward_error = None;
        self.forward_kind = crate::ssh_forward::ForwardKind::Local;
        self.settings = false;
        self.command_palette = false;
        self.editing_host = None;
        self.host_error_field = None;
        self.creating_tab = false;
        self.editing_session = None;
        self.home_task = None;
        self.directory_task = None;
        self.directory_candidates.clear();
        self.directory_error = None;
        self.validated_directory = None;

        self.message = None;
        self.need_focus = true;
        cx.notify();
    }
    fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings {
            window.close_all_dialogs(cx);
        } else {
            window.close_dialog(cx);
        }
        self.modal_closed(cx);
    }
    fn open_modal(
        &mut self,
        title: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = title.into();
        self.message = None;
        self.host_error_field = None;
        self.need_focus = false;
        let owner = cx.entity();
        let kind = if self.settings_ui.host_form {
            ModalKind::Host
        } else if self.forward_form {
            ModalKind::Forward
        } else if self.settings {
            ModalKind::Settings
        } else if self.command_palette {
            ModalKind::Commands
        } else {
            ModalKind::NewTab
        };
        let body = cx.new(|cx| ModalContent {
            owner: owner.downgrade(),
            kind,
            _subscription: cx.observe(&owner, |_, _, cx| cx.notify()),
        });
        let owner = owner.downgrade();
        let local_host = self.editing_host == Some(0);
        let forward_dialog_height = self.forward_dialog_height.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            let height: f32 = match kind {
                ModalKind::Settings => 720.,
                ModalKind::Host if local_host => 220.,
                ModalKind::Host => 460.,
                ModalKind::Commands => 410.,
                ModalKind::ThemeEditor => 460.,
                ModalKind::Forward => forward_dialog_height.get(),
                ModalKind::NewTab => 360.,
            };
            let height = height.min((f32::from(window.viewport_size().height) - 180.).max(180.));
            let width: f32 = match kind {
                ModalKind::Settings => 960.,
                ModalKind::Host => 560.,
                ModalKind::Commands => 620.,
                ModalKind::ThemeEditor => 640.,
                ModalKind::Forward => 780.,
                ModalKind::NewTab => 560.,
            };
            let width = width.min((f32::from(window.viewport_size().width) - 32.).max(0.));
            let owner = owner.clone();
            workspace_dialog(dialog, cx)
                .title(title.clone())
                .width(px(width))
                .margin_top(
                    if matches!(
                        kind,
                        ModalKind::Commands
                            | ModalKind::NewTab
                            | ModalKind::Host
                            | ModalKind::Forward
                    ) {
                        px(72.)
                    } else {
                        (window.viewport_size().height - px(height + 80.)) / 2.
                    },
                )
                .on_close(move |_, window, cx| {
                    let _ = owner.update(cx, |app, cx| match kind {
                        ModalKind::Host => {
                            app.host_editor_closed(cx);
                            if app.settings {
                                window.focus(&app.settings_ui.focus, cx);
                            }
                        }
                        _ => app.modal_closed(cx),
                    });
                })
                .child(if matches!(kind, ModalKind::NewTab) {
                    div()
                        .max_h(px(height))
                        .overflow_y_scrollbar()
                        .child(body.clone())
                        .into_any_element()
                } else {
                    div()
                        .h(px(height))
                        .min_h_0()
                        .child(body.clone())
                        .into_any_element()
                })
        });
        cx.notify();
    }
    fn show_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = true;
        self.command_palette = false;
        self.message = None;
        self.need_focus = false;
        self.refresh_keys(cx);
        self.open_modal(crate::t!("settings.title"), window, cx);
        window.focus(&self.settings_ui.focus, cx);
        cx.notify();
    }
    fn show_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_ui.host_form {
            return;
        }
        self.settings_ui.host_form = true;
        self.editing_host = None;
        self.creating_tab = false;

        self.label.update(cx, |i, cx| i.set_value("", window, cx));
        self.destination
            .update(cx, |i, cx| i.set_value("", window, cx));
        self.user.update(cx, |i, cx| i.set_value("", window, cx));
        self.port.update(cx, |i, cx| i.set_value("", window, cx));
        self.identity_file
            .update(cx, |i, cx| i.set_value("", window, cx));
        self.use_tmux = true;
        self.open_modal(crate::t!("ws.add_host_title"), window, cx);
        self.destination.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }
    fn refresh_forward_dialog_height(&self) {
        let count: usize = self.forwards.values().map(Vec::len).sum();
        self.forward_dialog_height.set(
            (count as f32 * 48. + if self.forward_add_form { 116. } else { 64. }).clamp(160., 380.),
        );
    }

    fn open_forward_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = false;
        self.command_palette = false;
        self.forward_form = true;
        self.forward_add_form = self.forwards.values().all(Vec::is_empty);
        self.forward_host_index = self.hosts.iter().position(|host| host.config.is_some());
        self.forward_kind = crate::ssh_forward::ForwardKind::Local;
        self.forward_error = None;
        self.forward_local_port
            .update(cx, |input, cx| input.set_value("8080", window, cx));
        self.forward_target_port
            .update(cx, |input, cx| input.set_value("80", window, cx));
        self.refresh_forward_dialog_height();
        self.open_modal(crate::t!("ssh.forward_title"), window, cx);
        if self.forward_add_form {
            self.forward_local_port
                .update(cx, |input, cx| input.focus(window, cx));
        }
    }

    fn add_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.forward_host_index else {
            self.forward_error = Some(crate::t!("ssh.forward_no_host").to_string());
            cx.notify();
            return;
        };
        let Some(config) = self.hosts.get(index).and_then(|host| host.config.clone()) else {
            self.forward_error = Some(crate::t!("ssh.forward_no_host").to_string());
            cx.notify();
            return;
        };
        let parse_port = |input: &Entity<InputState>| {
            input
                .read(cx)
                .value()
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
        };
        let Some(local_port) = parse_port(&self.forward_local_port) else {
            self.forward_error = Some(crate::t!("ssh.forward_invalid_port").to_string());
            self.forward_local_port
                .update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
            return;
        };
        let target_port = if self.forward_kind == crate::ssh_forward::ForwardKind::Dynamic {
            0
        } else if let Some(port) = parse_port(&self.forward_target_port) {
            port
        } else {
            self.forward_error = Some(crate::t!("ssh.forward_invalid_port").to_string());
            self.forward_target_port
                .update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
            return;
        };
        let forward = crate::ssh_forward::LocalForward {
            kind: self.forward_kind,
            bind_host: "127.0.0.1".into(),
            local_port,
            target_host: "localhost".into(),
            target_port,
        };
        if let Err(error) = forward.validate() {
            self.forward_error = Some(crate::i18n::format_error(&error));
            cx.notify();
            return;
        }
        let key = forward_storage_key(&config);
        let (rule_index, added) = {
            let rules = self.forwards.entry(key).or_default();
            if let Some(index) = rules.iter().position(|rule| rule == &forward) {
                (index, false)
            } else {
                rules.push(forward);
                (rules.len() - 1, true)
            }
        };
        if added {
            self.save(cx);
        }
        self.start_forward(&config, rule_index);
        self.finish_forward_add(window, cx);
    }

    fn finish_forward_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.forward_add_form = false;
        self.forward_error = None;
        self.refresh_forward_dialog_height();
        // The dialog's close action must not target an input removed with the draft row.
        window.focus(&self.forward_focus, cx);
        cx.notify();
    }

    fn toggle_forward(
        &mut self,
        host_index: usize,
        rule_index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(config) = self
            .hosts
            .get(host_index)
            .and_then(|host| host.config.clone())
        else {
            return;
        };
        let key = forward_storage_key(&config);
        let process_key = (key.clone(), rule_index);
        if let Some(mut process) = self.forward_processes.remove(&process_key) {
            let active = process.is_active();
            process.stop();
            if active {
                cx.notify();
                return;
            }
        }
        self.start_forward(&config, rule_index);
        cx.notify();
    }

    fn start_forward(&mut self, config: &HostDefinition, rule_index: usize) {
        let key = forward_storage_key(config);
        if self
            .forward_processes
            .contains_key(&(key.clone(), rule_index))
        {
            return;
        }
        let Some(forward) = self
            .forwards
            .get(&key)
            .and_then(|rules| rules.get(rule_index))
            .cloned()
        else {
            return;
        };
        match crate::ssh_forward::Tunnel::start(
            &config.ssh,
            &forward,
            self.workspace_updates.clone(),
        ) {
            Ok(process) => {
                self.forward_processes.insert((key, rule_index), process);
                self.forward_error = None;
            }
            Err(error) => {
                self.forward_error = Some(
                    crate::t!(
                        "ssh.forward_error",
                        error = crate::i18n::format_error(&error)
                    )
                    .to_string(),
                );
            }
        }
    }

    fn start_forwards_for(&mut self, config: &HostDefinition) {
        let key = forward_storage_key(config);
        let count = self.forwards.get(&key).map_or(0, Vec::len);
        for rule_index in 0..count {
            if self
                .forward_processes
                .get(&(key.clone(), rule_index))
                .is_some_and(|tunnel| {
                    matches!(tunnel.phase(), crate::ssh_forward::Phase::Failed(_))
                })
            {
                self.forward_processes.remove(&(key.clone(), rule_index));
            }
            self.start_forward(config, rule_index);
        }
    }

    fn stop_forwards_for(&mut self, config: &HostDefinition) {
        let key = forward_storage_key(config);
        let process_keys: Vec<_> = self
            .forward_processes
            .keys()
            .filter(|(host_key, _)| host_key == &key)
            .cloned()
            .collect();
        for process_key in process_keys {
            if let Some(mut process) = self.forward_processes.remove(&process_key) {
                process.stop();
            }
        }
    }

    fn remove_forward(&mut self, host_index: usize, rule_index: usize, cx: &mut Context<Self>) {
        let Some(config) = self
            .hosts
            .get(host_index)
            .and_then(|host| host.config.clone())
        else {
            return;
        };
        let key = forward_storage_key(&config);
        if !self
            .forwards
            .get(&key)
            .is_some_and(|rules| rule_index < rules.len())
        {
            return;
        }
        if let Some(mut tunnel) = self.forward_processes.remove(&(key.clone(), rule_index)) {
            tunnel.stop();
        }
        let later_keys: Vec<_> = self
            .forward_processes
            .keys()
            .filter(|(host_key, index)| host_key == &key && *index > rule_index)
            .cloned()
            .collect();
        for process_key in later_keys {
            if let Some(tunnel) = self.forward_processes.remove(&process_key) {
                self.forward_processes
                    .insert((key.clone(), process_key.1 - 1), tunnel);
            }
        }
        if let Some(rules) = self.forwards.get_mut(&key) {
            if rule_index < rules.len() {
                rules.remove(rule_index);
            }
            if rules.is_empty() {
                self.forwards.remove(&key);
            }
        }
        self.refresh_forward_dialog_height();
        self.save(cx);
        cx.notify();
    }

    fn forward_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use crate::ssh_forward::{ForwardKind, Phase};
        let p = self.palette;
        let hosts: Vec<_> = self
            .hosts
            .iter()
            .enumerate()
            .filter_map(|(index, host)| {
                host.config
                    .as_ref()
                    .map(|config| (index, host.name.clone(), config.clone()))
            })
            .collect();
        let rules: Vec<_> = hosts
            .iter()
            .flat_map(|(host_index, name, config)| {
                let key = forward_storage_key(config);
                self.forwards
                    .get(&key)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(move |(index, rule)| {
                        (*host_index, name.clone(), key.clone(), index, rule.clone())
                    })
            })
            .collect();
        let kind_label = |kind| match kind {
            ForwardKind::Local => crate::t!("ssh.forward_local").to_string(),
            ForwardKind::Remote => crate::t!("ssh.forward_remote").to_string(),
            ForwardKind::Dynamic => crate::t!("ssh.forward_dynamic").to_string(),
        };
        let selected = self.forward_host_index;
        let selected_name = hosts
            .iter()
            .find(|(index, _, _)| Some(*index) == selected)
            .map(|(_, name, _)| name.clone())
            .unwrap_or_else(|| crate::t!("ssh.forward_select_host").to_string());
        let kind = self.forward_kind;
        let owner = cx.entity().downgrade();
        let host_owner = owner.clone();
        let body = div()
            .id("forward-editor-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .px_2()
                    .text_size(px(12.))
                    .text_color(rgb(p.muted))
                    .child(
                        div()
                            .w(px(88.))
                            .flex_shrink_0()
                            .child(crate::t!("ssh.forward_type")),
                    )
                    .child(
                        div()
                            .w(px(160.))
                            .flex_shrink_0()
                            .child(crate::t!("ssh.forward_ssh_host")),
                    )
                    .child(div().flex_1().child(crate::t!("ssh.forward_ports")))
                    .child(
                        div()
                            .w(px(142.))
                            .flex_shrink_0()
                            .child(crate::t!("ssh.forward_state")),
                    ),
            )
            .children(rules.iter().enumerate().map(
                |(row_index, (host_index, name, key, rule_index, rule))| {
                    let tunnel = self.forward_processes.get(&(key.clone(), *rule_index));
                    let phase = tunnel
                        .map(|tunnel| tunnel.phase())
                        .unwrap_or(Phase::Stopped);
                    let active = tunnel.is_some_and(|tunnel| tunnel.is_active());
                    let counters = tunnel.map(|tunnel| tunnel.counters()).unwrap_or_default();
                    let status = match &phase {
                        Phase::Connecting => crate::t!("ws.connecting"),
                        Phase::Running => crate::t!("ssh.forward_running"),
                        Phase::Failed(_) => crate::t!("ssh.forward_failed"),
                        Phase::Stopped => crate::t!("ssh.forward_stopped"),
                    }
                    .to_string();
                    let detail = match phase {
                        Phase::Failed(error) => tshell_ui::i18n::format_failure(&error),
                        _ => format!(
                            "{} · {}",
                            rule.label(),
                            crate::t!(
                                "ssh.forward_bytes",
                                sent = counters.0,
                                received = counters.1
                            )
                        ),
                    };
                    let ports = match rule.kind {
                        ForwardKind::Dynamic => rule.local_port.to_string(),
                        _ => format!("{} → {}", rule.local_port, rule.target_port),
                    };
                    let host_index = *host_index;
                    let rule_index = *rule_index;
                    let toggle_owner = owner.clone();
                    let remove_owner = owner.clone();
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_h(px(44.))
                        .px_2()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .child(
                            div()
                                .w(px(88.))
                                .flex_shrink_0()
                                .text_size(px(12.))
                                .child(kind_label(rule.kind)),
                        )
                        .child(
                            div()
                                .w(px(160.))
                                .flex_shrink_0()
                                .truncate()
                                .child(name.clone()),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(ports))
                        .child(
                            div()
                                .id(("forward-state", row_index))
                                .w(px(58.))
                                .flex_shrink_0()
                                .text_size(px(11.))
                                .text_color(rgb(match tunnel.map(|tunnel| tunnel.phase()) {
                                    Some(Phase::Failed(_)) => p.error,
                                    _ if active => p.accent,
                                    _ => p.muted,
                                }))
                                .child(status)
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(detail.clone())
                                        .build(window, cx)
                                }),
                        )
                        .child(
                            Button::new(("toggle-forward", row_index))
                                .small()
                                .ghost()
                                .label(if active {
                                    crate::t!("ssh.forward_stop")
                                } else {
                                    crate::t!("ssh.forward_start")
                                })
                                .on_click(move |_, window, cx| {
                                    let _ = toggle_owner.update(cx, |app, cx| {
                                        app.toggle_forward(host_index, rule_index, window, cx)
                                    });
                                }),
                        )
                        .child(
                            icon_button(
                                ("remove-forward", row_index),
                                IconName::Trash,
                                crate::t!("ws.remove"),
                            )
                            .on_click(move |_, _, cx| {
                                let _ = remove_owner.update(cx, |app, cx| {
                                    app.remove_forward(host_index, rule_index, cx)
                                });
                            }),
                        )
                        .into_any_element()
                },
            ))
            .when(self.forward_add_form, |view| {
                view.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_h(px(44.))
                        .px_2()
                        .child(
                            Button::new("forward-kind")
                                .small()
                                .w(px(88.))
                                .label(kind_label(kind))
                                .dropdown_caret(true)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for next_kind in [
                                        ForwardKind::Local,
                                        ForwardKind::Remote,
                                        ForwardKind::Dynamic,
                                    ] {
                                        let owner = owner.clone();
                                        let label = match next_kind {
                                            ForwardKind::Local => crate::t!("ssh.forward_local"),
                                            ForwardKind::Remote => crate::t!("ssh.forward_remote"),
                                            ForwardKind::Dynamic => {
                                                crate::t!("ssh.forward_dynamic")
                                            }
                                        };
                                        menu = menu.item(
                                            PopupMenuItem::new(label.into_owned())
                                                .checked(kind == next_kind)
                                                .on_click(move |_, _, cx| {
                                                    let _ = owner.update(cx, |app, cx| {
                                                        app.forward_kind = next_kind;
                                                        cx.notify();
                                                    });
                                                }),
                                        );
                                    }
                                    menu
                                }),
                        )
                        .child(
                            Button::new("forward-host")
                                .small()
                                .w(px(160.))
                                .min_w_0()
                                .child(div().min_w_0().truncate().child(selected_name))
                                .dropdown_caret(true)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for (index, name, _) in &hosts {
                                        let index = *index;
                                        let owner = host_owner.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(name.clone())
                                                .checked(selected == Some(index))
                                                .on_click(move |_, _, cx| {
                                                    let _ = owner.update(cx, |app, cx| {
                                                        app.forward_host_index = Some(index);
                                                        cx.notify();
                                                    });
                                                }),
                                        );
                                    }
                                    menu
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    Input::new(&self.forward_local_port).small().aria_label(
                                        crate::t!("ssh.forward_listen_port").to_string(),
                                    ),
                                )
                                .when(kind != ForwardKind::Dynamic, |row| {
                                    row.child(div().text_color(rgb(p.muted)).child("→")).child(
                                        Input::new(&self.forward_target_port).small().aria_label(
                                            crate::t!("ssh.forward_target_port").to_string(),
                                        ),
                                    )
                                }),
                        )
                        .child(
                            Button::new("add-forward")
                                .small()
                                .primary()
                                .label(crate::t!("ssh.forward_save"))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.add_forward(window, cx)),
                                ),
                        )
                        .child(
                            icon_button(
                                "cancel-add-forward",
                                IconName::X,
                                crate::t!("ssh.forward_cancel"),
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.finish_forward_add(window, cx);
                                },
                            )),
                        ),
                )
            })
            .when_some(self.forward_error.clone(), |view, error| {
                view.child(style::notice(IconName::CircleAlert, error, p.error))
            });
        div()
            .size_full()
            .min_h_0()
            .track_focus(&self.forward_focus)
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .flex_shrink_0()
                    .child(
                        Button::new("show-add-forward")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .label(crate::t!("ssh.forward_add"))
                            .disabled(self.forward_add_form)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.forward_add_form = true;
                                this.forward_error = None;
                                this.forward_host_index =
                                    this.hosts.iter().position(|host| host.config.is_some());
                                this.refresh_forward_dialog_height();
                                this.forward_local_port
                                    .update(cx, |input, cx| input.focus(window, cx));
                                cx.notify();
                            })),
                    ),
            )
            .child(body)
            .into_any_element()
    }

    fn show_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let already_open = self.command_palette && window.has_active_dialog(cx);
        self.command_palette = true;
        self.settings = false;
        self.command_state
            .update(cx, |state, cx| state.set_query("", window, cx));
        if !already_open {
            self.open_modal(crate::t!("ws.launcher_title"), window, cx);
        }
        self.command_state
            .update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }
    fn show_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hosts[self.active].snapshot.session().is_none() {
            self.show_new_session(window, cx);
            return;
        }
        self.act(Action::NewWindow, cx);
        self.show_terminal(cx);
    }
    pub(super) fn show_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.editing_session = None;
        self.creating_tab = true;
        self.settings = false;
        self.command_palette = false;
        self.tab_path
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.directory_candidates.clear();
        self.directory_error = None;
        self.validated_directory = None;
        self.open_modal(crate::t!("ws.new_session"), window, cx);
        self.tab_name
            .update(cx, |input, cx| input.focus(window, cx));
        if let Some(remote) = self.remote_files() {
            self.home_task = Some(cx.spawn_in(window, async move |view, cx| {
                let home = cx
                    .background_executor()
                    .spawn(async move { remote_files::directory(&remote, "~") })
                    .await;
                let _ = view.update_in(cx, |this, window, cx| {
                    if !this.creating_tab
                        || this.editing_session.is_some()
                        || !this.tab_path.read(cx).value().is_empty()
                    {
                        return;
                    }
                    match home {
                        Ok(home) => {
                            this.tab_path
                                .update(cx, |input, cx| input.set_value(home, window, cx));
                            this.complete_tab_directory(cx);
                        }
                        Err(error) => {
                            this.directory_error = Some(
                                crate::t!(
                                    "ws.home_failed",
                                    error = crate::i18n::format_error(&error)
                                )
                                .to_string(),
                            );
                            cx.notify();
                        }
                    }
                });
            }));
        } else {
            let path = dirs::home_dir().unwrap_or_else(|| self.default_cwd.clone());
            self.tab_path.update(cx, |input, cx| {
                input.set_value(path.to_string_lossy().into_owned(), window, cx)
            });
            self.complete_tab_directory(cx);
        }
    }
    fn create_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.tab_path.read(cx).value().trim().to_owned();
        if path.is_empty() || path.chars().any(char::is_control) {
            self.message = Some(crate::t!("ws.directory_invalid").to_string());
            cx.notify();
            return;
        }
        let path = if self.editing_session.is_some() {
            path
        } else if self.session_directory(cx).is_none() {
            // Directory completion runs asynchronously. When the user presses
            // Enter immediately after choosing or typing a local path, resolve
            // it synchronously so a valid path is not rejected just because the
            // completion task has not published its result yet.
            if self.hosts[self.active].config.is_none() {
                let resolved = directories::resolve(&path, &self.cwd);
                match resolved.canonicalize() {
                    Ok(resolved) if resolved.is_dir() => resolved.to_string_lossy().into_owned(),
                    _ => {
                        self.message = Some(crate::t!("ws.directory_missing").to_string());
                        cx.notify();
                        return;
                    }
                }
            } else {
                self.message = Some(crate::t!("ws.directory_missing").to_string());
                cx.notify();
                return;
            }
        } else if self.hosts[self.active].config.is_none() {
            let path = directories::resolve(&path, &self.cwd);
            match path.canonicalize() {
                Ok(path) if path.is_dir() => path.to_string_lossy().into_owned(),
                _ => {
                    self.message = Some(crate::t!("ws.directory_missing").to_string());
                    cx.notify();
                    return;
                }
            }
        } else {
            self.session_directory(cx).unwrap().to_owned()
        };
        let name = self.tab_name.read(cx).value().trim().to_owned();
        if name.chars().any(char::is_control) {
            self.message = Some(crate::t!("ws.tab_name_invalid").to_string());
            cx.notify();
            return;
        }
        if self.hosts[self.active]
            .config
            .as_ref()
            .is_some_and(|host| host.tmux)
            && name.contains([':', '.'])
        {
            self.message = Some(crate::t!("ws.session_name_colon").to_string());
            cx.notify();
            return;
        }
        let action = match self.editing_session.clone() {
            Some(id) => {
                if name.is_empty() {
                    self.message = Some(crate::t!("ws.session_name_empty").to_string());
                    cx.notify();
                    return;
                }
                Action::RenameSession { id, name }
            }
            None => Action::NewNamedSession {
                name: sessions::session_name(&name, &path),
                path,
            },
        };
        self.close_modal(window, cx);
        self.act(action, cx);
    }
    fn session_directory(&self, cx: &App) -> Option<&str> {
        let input = self.tab_path.read(cx).value();
        self.validated_directory
            .as_ref()
            .filter(|(host, query, _)| *host == self.active && query == input.as_ref())
            .map(|(_, _, path)| path.as_str())
    }
    fn complete_tab_directory(&mut self, cx: &mut Context<Self>) {
        self.directory_candidates.clear();
        self.directory_error = None;
        self.validated_directory = None;
        self.message = None;
        let input = self.tab_path.read(cx).value().to_string();
        let host_index = self.active;
        let base = self.cwd.clone();
        let remote = self.remote_files();
        let checkable = !input.trim().is_empty() && !input.chars().any(char::is_control);
        if remote.is_none() && checkable {
            let path = directories::resolve(input.trim(), &base);
            self.validated_directory =
                path.canonicalize()
                    .ok()
                    .filter(|path| path.is_dir())
                    .map(|path| {
                        (
                            host_index,
                            input.clone(),
                            path.to_string_lossy().into_owned(),
                        )
                    });
            if self.validated_directory.is_none() {
                self.directory_error = Some(crate::t!("ws.directory_missing").to_string());
            }
        }
        self.directory_task = Some(cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let query = input.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let checked = remote
                        .as_ref()
                        .filter(|_| checkable)
                        .map(|host| remote_files::directory(host, &query));
                    let candidates = directories::complete(&query, &base, remote.as_ref());
                    (checked, candidates)
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                if this.creating_tab
                    && this.active == host_index
                    && this.tab_path.read(cx).value().as_ref() == input
                {
                    if let Some(checked) = result.0 {
                        match checked {
                            Ok(path) => {
                                this.validated_directory = Some((host_index, input.clone(), path))
                            }
                            Err(error) => {
                                this.directory_error = Some(crate::i18n::format_error(&error))
                            }
                        }
                    }
                    match result.1 {
                        Ok(paths) => this.directory_candidates = paths,
                        Err(error) if this.directory_error.is_none() => {
                            this.directory_error = Some(
                                crate::t!(
                                    "ws.completion_unavailable",
                                    error = crate::i18n::format_error(&error)
                                )
                                .to_string(),
                            )
                        }
                        Err(_) => {}
                    }
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }
    fn accept_directory(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_path.update(cx, |input, cx| {
            input.set_value(path, window, cx);
            input.focus(window, cx);
        });
        self.complete_tab_directory(cx);
    }
    fn browse_tab_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(crate::t!("ws.choose_directory").into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = view.update_in(cx, |this, window, cx| {
                        if this.creating_tab {
                            this.tab_path.update(cx, |input, cx| {
                                input.set_value(path.to_string_lossy().into_owned(), window, cx)
                            });
                        }
                    });
                }
            }
        })
        .detach();
    }
    fn browse_identity_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(crate::t!("ws.field_identity_file").into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = view.update_in(cx, |this, window, cx| {
                        if this.settings && this.settings_ui.host_form {
                            this.identity_file.update(cx, |input, cx| {
                                input.set_value(path.to_string_lossy().into_owned(), window, cx)
                            });
                        }
                    });
                }
            }
        })
        .detach();
    }
    fn switch_host_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.hosts.len() < 2 {
            return;
        }
        let len = self.hosts.len() as isize;
        let index = (self.active as isize + delta).rem_euclid(len) as usize;
        self.switch_host(index, cx);
    }
    fn font(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.font_size = (self.font_size + delta).clamp(10., 24.);
        for h in &mut self.hosts {
            h.viewport = None;
            for v in h.views.values() {
                v.update(cx, |t, cx| {
                    t.font_size = self.font_size;
                    cx.notify();
                });
            }
        }
        self.save(cx);
        cx.notify();
    }
    fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        self.apply_opacity(window, cx);
        self.save(cx);
        cx.notify();
    }
    fn set_keyboard_focus(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.keyboard_focus == enabled {
            return;
        }
        self.keyboard_focus = enabled;
        Theme::global_mut(cx).ring = rgb(if enabled {
            self.palette.accent
        } else {
            self.palette.border
        })
        .into();
        cx.notify();
    }

    fn keyboard(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == Page::Git
            && !self.settings
            && !self.command_palette
            && !self.creating_tab
            && event.keystroke.key.eq_ignore_ascii_case("c")
            && event.keystroke.modifiers.control
            && !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.platform
        {
            let text = gpui_kit::base::TextSelection::selected_text(window, cx);
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                cx.stop_propagation();
                window.prevent_default();
                return;
            }
        }
        if self.settings && self.settings_ui.recording.is_some() {
            self.record_shortcut(event, window, cx);
            return;
        }
        if event.keystroke.key == "escape" && self.settings && self.settings_ui.host_form {
            self.cancel_host_editor(window, cx);
            cx.stop_propagation();
            window.prevent_default();
            return;
        }
        if event.keystroke.key == "escape"
            && (!window.has_active_dialog(cx) || self.settings)
            && (self.creating_tab || self.settings || self.command_palette)
        {
            self.close_modal(window, cx);
            cx.stop_propagation();
            window.prevent_default();
            return;
        }
        if self.page == Page::Explorer
            && self.explorer_entity().is_some_and(|entity| {
                let explorer = entity.read(cx);
                explorer.open_file.is_some()
                    && explorer
                        .file_editor
                        .read(cx)
                        .focus_handle(cx)
                        .contains_focused(window, cx)
            })
            && event.keystroke.key.eq_ignore_ascii_case("s")
            && event.keystroke.modifiers.control
            && !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.shift
            && !event.keystroke.modifiers.platform
            && !self.creating_tab
            && !self.settings
            && !self.command_palette
        {
            self.save_open_file(window, cx);
            cx.stop_propagation();
            window.prevent_default();
            return;
        }
        let Some(shortcut) = self.shortcut_map.resolve(&event.keystroke) else {
            return;
        };
        if self.creating_tab || self.settings || self.command_palette {
            return;
        }
        self.execute_shortcut(shortcut, window, cx);
        cx.stop_propagation();
        window.prevent_default();
    }
    fn execute_shortcut(
        &mut self,
        shortcut: Shortcut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match shortcut {
            Shortcut::CommandPalette => {
                self.show_command_palette(window, cx);
            }
            Shortcut::TerminalSearch => {
                self.show_terminal(cx);
                if let Some(w) = self.hosts[self.active].snapshot.window()
                    && let Some(view) = self.hosts[self.active].views.get(&w.active_pane)
                {
                    view.update(cx, |view, cx| view.open_search(window, cx));
                }
            }
            Shortcut::Copy | Shortcut::Paste => {
                if self.page == Page::Git {
                    if shortcut == Shortcut::Copy {
                        let text = gpui_kit::base::TextSelection::selected_text(window, cx);
                        if !text.is_empty() {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                    }
                    return;
                }
                if let Some(w) = self.hosts[self.active].snapshot.window() {
                    if let Some(view) = self.hosts[self.active].views.get(&w.active_pane) {
                        view.update(cx, |view, cx| {
                            if shortcut == Shortcut::Copy {
                                view.copy(cx);
                            } else {
                                view.paste(cx);
                            }
                        });
                    }
                }
            }
            Shortcut::ShowTerminal => self.show_terminal(cx),
            Shortcut::ShowFiles => self.show_files(cx),
            Shortcut::ShowGit => self.show_git(cx),
            Shortcut::NewWindow => self.show_new_tab(window, cx),
            Shortcut::CloseWindow => self.act(Action::CloseWindow, cx),
            Shortcut::AddHost => self.show_add(window, cx),
            Shortcut::EditHost => self.edit_host(self.active, window, cx),
            Shortcut::RemoveHost => self.remove_host(self.active, cx),
            Shortcut::NextHost => self.switch_host_relative(1, cx),
            Shortcut::PreviousHost => self.switch_host_relative(-1, cx),
            Shortcut::RefreshFiles => self.refresh_files(window, cx),
            Shortcut::RefreshGit => self.refresh_git(cx),
            Shortcut::Reconnect => self.reconnect(cx),
            Shortcut::SaveFile => self.save_open_file(window, cx),
            Shortcut::Settings => self.show_settings(window, cx),
            Shortcut::CycleTheme => {
                self.set_appearance(self.appearance.next(), window, cx);
            }
            Shortcut::ClosePane => self.act(Action::ClosePane, cx),
            Shortcut::ToggleSidebar => self.toggle_sidebar(window, cx),
            Shortcut::Zen => {
                self.zen = !self.zen;
                self.apply_opacity(window, cx);
                self.sync_latency_monitor(window, cx);
            }
            Shortcut::RenderStats => window.cycle_debug_frame_overlay_mode(),
            Shortcut::Focus => self.act(Action::Zoom, cx),
            Shortcut::Floating => {
                self.floating = !self.floating;
            }
            Shortcut::Font(n) => self.font(n as f32, cx),
            Shortcut::NewColumn => self.act(Action::Split(SplitAxis::Horizontal), cx),
            Shortcut::NewRow => self.act(Action::Split(SplitAxis::Vertical), cx),
            Shortcut::Move(d) => self.act(Action::MoveFocus(d), cx),
            Shortcut::MovePane(d) => self.act(Action::MovePane(d), cx),
            Shortcut::SelectSession(index) => self.select_session_at(index, cx),
            Shortcut::Session(i) => {
                if i < self.hosts.len() {
                    self.switch_host(i, cx);
                }
            }
        }
        cx.notify();
    }
    fn edit_host(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_ui.host_form || index >= self.hosts.len() {
            return;
        }
        self.settings_ui.host_form = true;
        self.editing_host = Some(index);

        let h = &self.hosts[index];
        let name = h.name.clone();
        let config = h.config.clone();
        self.label.update(cx, |i, cx| {
            i.set_value(name, window, cx);
        });
        if let Some(c) = config {
            self.destination
                .update(cx, |i, cx| i.set_value(c.ssh.destination, window, cx));
            self.user
                .update(cx, |i, cx| i.set_value(c.ssh.user, window, cx));
            self.port.update(cx, |i, cx| {
                i.set_value(
                    c.ssh.port.map(|p| p.to_string()).unwrap_or_default(),
                    window,
                    cx,
                )
            });
            self.identity_file.update(cx, |i, cx| {
                i.set_value(
                    c.ssh
                        .identity_file
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    window,
                    cx,
                )
            });
            self.use_tmux = c.tmux;
        }
        self.open_modal(crate::t!("ws.edit_host_title"), window, cx);
        self.label.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }
    fn mark_terminal_read(&mut self, id: &str, cx: &mut Context<Self>) {
        let count = self.hosts[self.active]
            .snapshot
            .windows()
            .flat_map(|tab| &tab.panes)
            .find(|pane| pane.id == id)
            .map(|pane| pane.notice_count);
        if let Some(count) = count {
            self.hosts[self.active]
                .read_notices
                .insert(id.to_owned(), count);
            cx.notify();
        }
    }
    fn host_files_busy(&self, index: usize, cx: &App) -> bool {
        let id = self.hosts[index].identity();
        self.docker.read(cx).busy(&id, cx)
            || self.workspaces.iter().any(|((host, _), workspace)| {
                *host == id
                    && workspace
                        .explorer
                        .as_ref()
                        .is_some_and(|explorer| explorer.read(cx).busy())
            })
    }
    fn session_files_busy(&self, session_id: &str, cx: &App) -> bool {
        self.workspaces
            .get(&(self.hosts[self.active].identity(), session_id.to_owned()))
            .and_then(|workspace| workspace.explorer.as_ref())
            .is_some_and(|explorer| explorer.read(cx).busy())
    }
    fn closes_session_with_unsaved_files(&self, action: &Action, cx: &App) -> bool {
        let snapshot = &self.hosts[self.active].snapshot;
        let session_id = match action {
            Action::RemoveSession(id) => Some(id.as_str()),
            Action::CloseWindow => snapshot
                .session()
                .filter(|session| session.windows.len() == 1)
                .map(|session| session.id.as_str()),
            Action::ClosePane => snapshot
                .session()
                .filter(|session| session.windows.len() == 1 && session.windows[0].panes.len() == 1)
                .map(|session| session.id.as_str()),
            _ => None,
        };
        session_id.is_some_and(|id| self.session_files_busy(id, cx))
    }

    fn prepare_host_connection_change(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        if self.host_files_busy(index, cx) {
            self.message = Some(crate::t!("ws.host_files_busy").to_string());
            cx.notify();
            return false;
        }
        let id = self.hosts[index].identity();
        self.workspaces.retain(|(host, _), _| *host != id);
        if self.active == index {
            self.active_workspace = None;
            self.page = Page::Terminal;
        }
        true
    }

    fn remove_host(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == 0 || index >= self.hosts.len() {
            return;
        }
        if self.host_files_busy(index, cx) {
            self.message = Some(crate::t!("ws.host_files_busy").to_string());
            cx.notify();
            return;
        }
        if let Some(config) = self.hosts[index].config.clone() {
            self.stop_forwards_for(&config);
            self.forwards.remove(&forward_storage_key(&config));
        }
        let id = self.hosts[index].identity();
        self.workspaces.retain(|(host, _), _| *host != id);
        self.hosts.remove(index);
        if self
            .active_workspace
            .as_ref()
            .is_some_and(|(host, _)| *host == id)
        {
            self.active_workspace = None;
        }
        self.active = if self.active == index {
            0
        } else if self.active > index {
            self.active - 1
        } else {
            self.active
        };
        self.editing_host = None;

        self.need_focus = true;
        self.save(cx);
        self.sync(cx);
        cx.notify();
    }
    fn measure(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let shaped = window.text_system().shape_line(
            "M".into(),
            px(self.font_size),
            &[TextRun {
                len: 1,
                font: font(self.font_family.clone()),
                color: rgb(self.palette.text).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        self.cell_width = f32::from(shaped.width).max(1.);
        self.line_height =
            self.line_height_scale
                .measure(window, &self.font_family, self.font_size);
        let h = &mut self.hosts[self.active];
        if let Some(w) = h.snapshot.window() {
            let desired = (
                w.id.clone(),
                PixelViewport {
                    width: f32::from(bounds.size.width),
                    height: f32::from(bounds.size.height),
                    cell_width: self.cell_width,
                    line_height: self.line_height,
                    horizontal_padding: PANE_PADDING,
                    vertical_padding: if self.zen { PANE_PADDING } else { 0. },
                },
            );
            if h.viewport.as_ref() != Some(&desired) {
                h.viewport = Some(desired.clone());
                if let Some(b) = &mut h.backend {
                    match b {
                        Backend::Local(local) => local.resize_pixels(&desired.0, desired.1),
                        Backend::Tmux(_) => {
                            let (cols, rows) = desired.1.terminal_size();
                            b.resize(&desired.0, cols.max(12), rows.max(6));
                        }
                    }
                }
                // Layout measurement runs during draw; defer model sync to the UI task.
                let _ = self.workspace_updates.try_send(());
                cx.notify();
            }
        }
    }
    fn surface(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let owner = cx.entity();
        canvas(
            move |bounds, window, cx| {
                // Build pane geometry from this frame's bounds before any terminal is drawn.
                let mut content = owner.update(cx, |this, cx| {
                    this.measure(bounds, window, cx);
                    this.pane_surface(cx)
                });
                content.layout_as_root(bounds.size.into(), window, cx);
                content.prepaint_at(bounds.origin, window, cx);
                content
            },
            |_, mut content, window, cx| content.paint(window, cx),
        )
        .size_full()
        .into_any_element()
    }

    fn pane_surface(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let h = &self.hosts[self.active];
        let Some(w) = h.snapshot.window().cloned() else {
            let can_create = h.snapshot.session().is_some();
            return div()
                .size_full()
                .flex()
                .flex_col()
                .gap_3()
                .items_center()
                .justify_center()
                .text_color(rgb(self.palette.muted))
                .child(if h.snapshot.connected() {
                    crate::t!("ws.no_terminal")
                } else if h.connection() == Connection::Closed {
                    crate::t!("ws.disconnected")
                } else {
                    crate::t!("ws.connecting")
                })
                .when(h.snapshot.connected(), |view| {
                    view.child(
                        Button::new("empty-workspace-create")
                            .small()
                            .icon(IconName::Plus)
                            .label(if can_create {
                                crate::t!("ws.new_terminal")
                            } else {
                                crate::t!("ws.new_session")
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if can_create {
                                    this.act(Action::NewWindow, cx);
                                } else {
                                    this.show_new_session(window, cx);
                                }
                            })),
                    )
                })
                .into_any_element();
        };
        let pixel_layout = match (&h.backend, &h.viewport) {
            (Some(Backend::Local(local)), Some((id, viewport))) if id == &w.id => {
                Some((local.pixel_panes(&w, *viewport), *viewport))
            }
            _ => None,
        };
        let local_viewport = pixel_layout.as_ref().map(|(_, viewport)| *viewport);
        let (geometry, surface_width, surface_height, gap_x, gap_y, padding_x, padding_y) =
            if let Some((panes, viewport)) = pixel_layout {
                (
                    panes,
                    viewport.width,
                    viewport.height,
                    PANE_GAP,
                    PANE_GAP,
                    PANE_PADDING,
                    viewport.vertical_padding,
                )
            } else {
                (
                    w.panes
                        .iter()
                        .map(|p| PaneBounds {
                            id: p.id.clone(),
                            x: p.x as f32 * self.cell_width,
                            y: p.y as f32 * self.line_height,
                            width: p.cols as f32 * self.cell_width,
                            height: p.rows as f32 * self.line_height,
                        })
                        .collect(),
                    w.cols as f32 * self.cell_width,
                    w.rows as f32 * self.line_height,
                    self.cell_width,
                    self.line_height,
                    0.,
                    0.,
                )
            };
        let mut panes = Vec::new();
        let mut dividers = Vec::new();
        // Keep strokes opaque so overlapping junctions do not become darker.
        let divider_color = rgb(self.palette.border).blend(rgba((self.palette.muted << 8) | 0x40));
        let active_color = rgb(self.palette.border).blend(rgba((self.palette.accent << 8) | 0xb3));
        let highlights = geometry
            .iter()
            .find(|pane| pane.id == w.active_pane)
            .map(|pane| active_dividers(pane, surface_width, surface_height, gap_x, gap_y))
            .unwrap_or_default();
        for p in &geometry {
            let id = p.id.clone();
            let view = h.views.get(&id).cloned();
            let (left, top, width, height) = (p.x, p.y, p.width, p.height);
            let pane = w.panes.iter().find(|pane| pane.id == id);
            let content_size = if let Some(viewport) = local_viewport {
                Some(p.terminal_size(viewport))
            } else {
                pane.map(|pane| (pane.cols, pane.rows))
            };
            let (inset_left, inset_top, inset_right, inset_bottom) = content_size
                .map(|(cols, rows)| {
                    let left = leading_inset(width, cols, self.cell_width, padding_x);
                    let top = leading_inset(height, rows, self.line_height, padding_y);
                    (
                        left,
                        top,
                        (width - left - cols as f32 * self.cell_width).max(0.),
                        (height - top - rows as f32 * self.line_height).max(0.),
                    )
                })
                .unwrap_or((padding_x, padding_y, padding_x, padding_y));
            let content = view.map(|v| v.into_any_element()).unwrap_or_else(|| {
                div()
                    .text_color(rgb(self.palette.muted))
                    .p_3()
                    .child(crate::t!("ws.restoring"))
                    .into_any_element()
            });
            let click_id = id.clone();
            panes.push(
                div()
                    .id(SharedString::from(format!("pane-{id}")))
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(width))
                    .h(px(height))
                    .pl(px(inset_left))
                    .pt(px(inset_top))
                    .pr(px(inset_right))
                    .pb(px(inset_bottom))
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.mark_terminal_read(&click_id, cx);
                            if this.hosts[this.active]
                                .snapshot
                                .window()
                                .is_some_and(|w| w.active_pane != click_id)
                            {
                                this.act(Action::SelectPane(click_id.clone()), cx);
                            }
                        }),
                    )
                    .child(content),
            );
            for axis in [SplitAxis::Horizontal, SplitAxis::Vertical] {
                if !match axis {
                    SplitAxis::Horizontal => p.x + p.width < surface_width - 0.5,
                    SplitAxis::Vertical => p.y + p.height < surface_height - 0.5,
                } {
                    continue;
                }
                let (line_offset, line_length) = match axis {
                    SplitAxis::Horizontal => divider_span(top, height, surface_height, gap_y),
                    SplitAxis::Vertical => divider_span(left, width, surface_width, gap_x),
                };
                let (x, y, wd, ht) = match axis {
                    SplitAxis::Horizontal => (
                        left + width + (gap_x - PANE_STROKE) / 2.,
                        top + line_offset,
                        PANE_STROKE,
                        line_length,
                    ),
                    SplitAxis::Vertical => (
                        left + line_offset,
                        top + height + (gap_y - PANE_STROKE) / 2.,
                        line_length,
                        PANE_STROKE,
                    ),
                };
                dividers.push(Bounds::new(point(px(x), px(y)), size(px(wd), px(ht))));
            }
        }
        let (offset_x, offset_y) = if matches!(h.backend, Some(Backend::Tmux(_))) {
            h.viewport
                .as_ref()
                .filter(|(id, _)| id == &w.id)
                .map(|(_, viewport)| {
                    // Server snapshots can still have the previous window's size while resizing.
                    let (cols, rows) = viewport.terminal_size();
                    (
                        leading_inset(
                            viewport.width,
                            cols,
                            self.cell_width,
                            viewport.horizontal_padding,
                        ),
                        leading_inset(
                            viewport.height,
                            rows,
                            self.line_height,
                            viewport.vertical_padding,
                        ),
                    )
                })
                .unwrap_or((0., 0.))
        } else {
            (0., 0.)
        };
        div()
            .id("pane-surface")
            .relative()
            .size_full()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left(px(offset_x))
                    .top(px(offset_y))
                    .w(px(surface_width))
                    .h(px(surface_height))
                    .children(panes)
                    .when(geometry.len() > 1, |surface| {
                        let accent = active_color;
                        surface.child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, (), window, _| {
                                    for line in &dividers {
                                        window.paint_quad(fill(
                                            Bounds::new(bounds.origin + line.origin, line.size),
                                            divider_color,
                                        ));
                                    }
                                    for line in &highlights {
                                        window.paint_quad(fill(
                                            Bounds::new(bounds.origin + line.origin, line.size),
                                            accent,
                                        ));
                                    }
                                },
                            )
                            .absolute()
                            .size_full(),
                        )
                    }),
            )
            .into_any_element()
    }

    fn activity_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        div()
            .h(px(36.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(rgb(p.border))
            .text_color(rgb(p.muted))
            .child(
                icon_button(
                    "mode-terminal",
                    IconName::Terminal,
                    crate::t!("ws.mode_terminal_tip"),
                )
                .flex_1()
                .h(px(30.))
                .when(self.page == Page::Terminal, |button| {
                    button.bg(rgb(p.selected)).text_color(rgb(p.text))
                })
                .on_click(cx.listener(|this, _, _, cx| this.show_terminal(cx))),
            )
            .child(
                icon_button(
                    "mode-files",
                    IconName::FolderOpen,
                    crate::t!("ws.mode_files_tip"),
                )
                .flex_1()
                .h(px(30.))
                .when(self.page == Page::Explorer, |button| {
                    button.bg(rgb(p.selected)).text_color(rgb(p.text))
                })
                .on_click(cx.listener(|this, _, _, cx| this.show_files(cx))),
            )
            .child(
                icon_button(
                    "mode-git",
                    IconName::GitBranch,
                    crate::t!("ws.mode_git_tip"),
                )
                .flex_1()
                .h(px(30.))
                .when(self.page == Page::Git, |button| {
                    button.bg(rgb(p.selected)).text_color(rgb(p.text))
                })
                .on_click(cx.listener(|this, _, _, cx| this.show_git(cx))),
            )
            .child(
                icon_button("mode-docker", IconName::Box, crate::t!("docker.title"))
                    .flex_1()
                    .h(px(30.))
                    .when(self.page == Page::Docker, |button| {
                        button.bg(rgb(p.selected)).text_color(rgb(p.text))
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::Docker;
                        this.need_focus = true;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn host_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let menu_item = |label: String| {
            PopupMenuItem::element(move |_, _| {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(14.))
                    .line_height(relative(1.4))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label.clone())
            })
        };
        let current = self.active;
        let hosts: Vec<_> = self
            .hosts
            .iter()
            .enumerate()
            .map(|(index, host)| {
                (
                    index,
                    host.name.clone(),
                    host.config.is_some(),
                    host.connection(),
                )
            })
            .collect();
        let owner = cx.entity().downgrade();
        div()
            .px_2()
            .py_1()
            .flex_shrink_0()
            .child(
                Button::new("host-selector")
                    .ghost()
                    .small()
                    .w_full()
                    .h(px(32.))
                    .px_2()
                    .accessibility_label(
                        crate::t!(
                            "ws.switch_host_a11y",
                            name = self.hosts[current].name.clone()
                        )
                        .to_string(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_left()
                            .truncate()
                            .text_size(px(14.))
                            .line_height(relative(1.4))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.hosts[current].name.clone()),
                    )
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.min_w(px(220.));
                        for (index, name, remote, connection) in &hosts {
                            let index = *index;
                            let name = name.clone();
                            let connection = *connection;
                            let status = match connection {
                                Connection::Connecting => crate::t!("ws.connecting"),
                                Connection::Ready => crate::t!("ws.connected"),
                                Connection::Closed => crate::t!("ws.not_connected"),
                            }
                            .to_string();
                            let owner = owner.clone();
                            let disconnect_owner = owner.clone();
                            let show_disconnect = *remote && connection == Connection::Ready;
                            menu = menu.item(
                                PopupMenuItem::element(move |_, _| {
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(px(14.))
                                                .line_height(relative(1.4))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(name.clone()),
                                        )
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "host-status-{index}"
                                                )))
                                                .size(px(6.))
                                                .flex_none()
                                                .rounded_full()
                                                .bg(rgb(if connection == Connection::Ready {
                                                    p.accent
                                                } else {
                                                    p.muted
                                                }))
                                                .tooltip({
                                                    let status = status.clone();
                                                    move |window, cx| {
                                                        gpui_kit::component::tooltip::Tooltip::new(
                                                            status.clone(),
                                                        )
                                                        .build(window, cx)
                                                    }
                                                }),
                                        )
                                        .when(show_disconnect, |row| {
                                            let disconnect_owner = disconnect_owner.clone();
                                            row.child(
                                                icon_button(
                                                    SharedString::from(format!(
                                                        "disconnect-host-{index}"
                                                    )),
                                                    IconName::Unplug,
                                                    crate::t!("ws.disconnect"),
                                                )
                                                .w(px(22.))
                                                .h(px(22.))
                                                .flex_shrink_0()
                                                .on_click(move |_, window, cx| {
                                                    cx.stop_propagation();
                                                    let _ =
                                                        disconnect_owner.update(cx, |this, cx| {
                                                            this.disconnect_host(index, cx);
                                                        });
                                                    window.dispatch_action(
                                                        Box::new(gpui_kit::base::actions::Cancel),
                                                        cx,
                                                    );
                                                }),
                                            )
                                        })
                                })
                                .icon(if *remote {
                                    IconName::Server
                                } else {
                                    IconName::Monitor
                                })
                                .checked(index == current)
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        owner.update(cx, |this, cx| this.switch_host(index, cx));
                                }),
                            );
                        }
                        let add = owner.clone();
                        let edit = owner.clone();
                        menu.separator()
                            .item(
                                menu_item(crate::t!("ws.host_add_menu").into_owned())
                                    .icon(IconName::Plus)
                                    .on_click(move |_, window, cx| {
                                        let _ =
                                            add.update(cx, |this, cx| this.show_add(window, cx));
                                    }),
                            )
                            .item(
                                menu_item(crate::t!("ws.host_edit_menu").into_owned()).on_click(
                                    move |_, window, cx| {
                                        let _ = edit.update(cx, |this, cx| {
                                            this.edit_host(current, window, cx)
                                        });
                                    },
                                ),
                            )
                    }),
            )
            .into_any_element()
    }

    fn file_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        self.explorer_entity()
            .map(|entity| entity.update(cx, |explorer, cx| explorer.file_sidebar(cx)))
            .unwrap_or_else(|| div().into_any_element())
    }
    fn git_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        self.git_entity()
            .map(|entity| entity.update(cx, |git, cx| git.git_sidebar(cx)))
            .unwrap_or_else(|| div().into_any_element())
    }
    fn sidebar_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let title = match self.page {
            Page::Terminal => crate::t!("ws.sessions").to_string(),
            Page::Docker => crate::t!("docker.title").into_owned(),
            Page::Explorer | Page::Git => self
                .cwd
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.cwd.to_string_lossy().into_owned()),
        };
        let mut actions = Vec::new();
        match self.page {
            Page::Docker => actions.push(
                icon_button(
                    "docker-sidebar-refresh",
                    IconName::RefreshCw,
                    crate::t!("ws.refresh"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.docker.update(cx, |docker, cx| docker.refresh(cx))
                }))
                .into_any_element(),
            ),
            Page::Terminal => actions.push(
                icon_button(
                    "new-session",
                    IconName::Plus,
                    crate::t!("ws.new_session_tip"),
                )
                .h(px(22.))
                .w(px(22.))
                .on_click(cx.listener(|this, _, window, cx| this.show_new_session(window, cx)))
                .into_any_element(),
            ),
            Page::Explorer | Page::Git => {
                if self.page == Page::Explorer {
                    let owner = cx.entity().downgrade();
                    actions.push(
                        icon_button(
                            "explorer-menu",
                            IconName::Ellipsis,
                            crate::t!("file.actions"),
                        )
                        .h(px(22.))
                        .w(px(22.))
                        .dropdown_menu(move |menu, _, cx| {
                            let Some(app) = owner.upgrade() else {
                                return menu;
                            };
                            let app = app.read(cx);
                            if let Some(entity) = app.explorer_entity() {
                                let owner = entity.downgrade();
                                let explorer = entity.read(cx);
                                explorer.explorer_menu(
                                    menu,
                                    explorer.cwd.clone(),
                                    true,
                                    true,
                                    owner,
                                )
                            } else {
                                menu
                            }
                        })
                        .into_any_element(),
                    );
                }
                actions.push(
                    icon_button("refresh-tool", IconName::RefreshCw, crate::t!("ws.refresh"))
                        .h(px(22.))
                        .w(px(22.))
                        .on_click(cx.listener(|this, _, window, cx| match this.page {
                            Page::Explorer => this.refresh_files(window, cx),
                            Page::Git => this.refresh_git(cx),
                            Page::Terminal | Page::Docker => {}
                        }))
                        .into_any_element(),
                );
            }
        }
        div()
            .h(px(style::TOOLBAR_HEIGHT))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(13.))
            .text_color(rgb(self.palette.text))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .children(actions),
            )
            .into_any_element()
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let content = match self.page {
            Page::Terminal => self.session_list(cx),
            Page::Explorer => self.file_sidebar(cx),
            Page::Git => self.git_sidebar(cx),
            Page::Docker => self
                .docker
                .update(cx, |docker, cx| docker.sidebar(self.sidebar_width, cx)),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgba(
                (self.palette.panel << 8) | (self.sidebar_opacity * 255.).round() as u32,
            ))
            .border_r_1()
            .border_color(rgb(self.palette.border))
            .child(self.host_list(cx))
            .child(self.activity_switcher(cx))
            .when(self.page != Page::Docker, |view| {
                view.child(self.sidebar_header(cx))
            })
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }
    fn file_surface(&self, _cx: &mut Context<Self>) -> AnyElement {
        match self.page {
            Page::Explorer => self
                .explorer_entity()
                .map(|entity| entity.into_any_element()),
            Page::Git => self.git_entity().map(|entity| entity.into_any_element()),
            Page::Docker => Some(self.docker.clone().into_any_element()),
            Page::Terminal => None,
        }
        .unwrap_or_else(|| {
            style::empty_state(
                IconName::FolderOpen,
                self.navigation_loading
                    .clone()
                    .unwrap_or_else(|| crate::t!("git.select_file").to_string()),
                self.palette,
            )
            .into_any_element()
        })
    }
    fn sync_metrics(&mut self, cx: &mut Context<Self>) {
        let host = &self.hosts[self.active];
        let target = (host.snapshot.connected()
            && self.metrics_config.resources().next().is_some())
        .then(|| {
            host.config
                .clone()
                .map(|host| host_metrics::Target::Remote(host.ssh))
                .unwrap_or(host_metrics::Target::Local)
        });
        let selection: BTreeSet<_> = self.metrics_config.resources().collect();
        if self.metrics_target == target && self.metrics_selection == selection {
            return;
        }
        self.metrics_monitor = None;
        self.metrics_target = target.clone();
        self.metrics_selection = selection.clone();
        self.metrics = None;
        self.metrics_epoch += 1;
        let epoch = self.metrics_epoch;
        if let Some(target) = target {
            let (monitor, updates) = host_metrics::Monitor::start(target, selection);
            self.metrics_monitor = Some(monitor);
            cx.spawn(async move |view, cx| {
                while let Ok(stats) = updates.recv().await {
                    let keep = view
                        .update(cx, |this, cx| {
                            if this.metrics_epoch != epoch {
                                return false;
                            }
                            this.metrics = Some(stats);
                            cx.notify();
                            true
                        })
                        .unwrap_or(false);
                    if !keep {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }
    fn terminal_status_items(
        &self,
        snapshot: &Snapshot,
        side: metrics_config::Side,
    ) -> Vec<(String, String)> {
        let host = &self.hosts[self.active];
        let mut resource_error_shown = false;
        let format_latency = |value: Option<Duration>| {
            value
                .map(|duration| format!("{:.1} ms", duration.as_secs_f64() * 1000.))
                .unwrap_or_else(|| "--".to_string())
        };
        self.metrics_config
            .0
            .iter()
            .filter(|item| item.enabled && item.side() == side)
            .filter_map(|item| match item.metric {
                metric if metric.is_resource() => match &self.metrics {
                    Some(Ok(stats)) => stats.item(metric),
                    Some(Err(error)) if self.metrics_target.is_some() && !resource_error_shown => {
                        resource_error_shown = true;
                        Some((
                            crate::t!("ws.resource_unavailable").to_string(),
                            error.clone(),
                        ))
                    }
                    _ => None,
                },
                metrics_config::Metric::Geometry => snapshot.window().map(|window| {
                    let label =
                        crate::t!("ws.pane_summary", cols = window.cols, rows = window.rows)
                            .to_string();
                    (label.clone(), label)
                }),
                metrics_config::Metric::SshRtt if host.config.is_some() => {
                    let label = format!(
                        "{} {}",
                        item.metric.label(),
                        format_latency(self.connection_latency.ssh)
                    );
                    Some((label, crate::t!("metrics.ssh_rtt_detail").to_string()))
                }
                metrics_config::Metric::TmuxRtt
                    if matches!(host.backend, Some(Backend::Tmux(_))) =>
                {
                    let label = format!(
                        "{} {}",
                        item.metric.label(),
                        format_latency(self.connection_latency.tmux)
                    );
                    Some((label, crate::t!("metrics.tmux_rtt_detail").to_string()))
                }
                _ => None,
            })
            .collect()
    }
    fn terminal_status_strip(&self, snapshot: &Snapshot, side: metrics_config::Side) -> AnyElement {
        div()
            .id(match side {
                metrics_config::Side::Left => "terminal-status-left",
                metrics_config::Side::Right => "terminal-status-right",
            })
            .min_w_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .children(
                self.terminal_status_items(snapshot, side)
                    .into_iter()
                    .enumerate()
                    .map(|(index, (label, detail))| {
                        div()
                            .id(("status-item", index))
                            .min_w_0()
                            .flex()
                            .items_center()
                            .when(index > 0, |item| {
                                item.child(
                                    div()
                                        .w(px(1.))
                                        .h(px(9.))
                                        .mx(px(8.))
                                        .flex_shrink_0()
                                        .bg(rgba((self.terminal_palette.muted << 8) | 45)),
                                )
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_features(FontFeatures(std::sync::Arc::new(vec![(
                                        "tnum".into(),
                                        1,
                                    )])))
                                    .child(label),
                            )
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(detail.clone())
                                    .build(window, cx)
                            })
                    }),
            )
            .into_any_element()
    }
    fn forward_status_button(&self, cx: &Context<Self>) -> Option<AnyElement> {
        Some(
            icon_button(
                "forward-status-summary",
                IconName::Waypoints,
                crate::t!("ssh.forward_title"),
            )
            .h(px(CHROME_BAR_HEIGHT - 4.))
            .w(px(CHROME_BAR_HEIGHT - 4.))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_forward_editor(window, cx);
            }))
            .into_any_element(),
        )
    }

    fn status_bar(&self, snapshot: &Snapshot, window: &Window, cx: &Context<Self>) -> AnyElement {
        let line_height = chrome_line_height(window, 12.);
        let height = px(CHROME_BAR_HEIGHT);
        match self.page {
            Page::Docker => StatusBar::new()
                .h(height)
                .py_0()
                .px_3()
                .bg(rgb(self.palette.panel))
                .text_size(px(12.))
                .text_color(rgb(self.palette.muted))
                .left(self.docker.read(cx).status())
                .into_any_element(),
            Page::Git => StatusBar::new()
                .h(height)
                .min_h(height)
                .py_0()
                .px_3()
                .bg(rgb(self.palette.panel))
                .border_color(rgb(self.palette.border))
                .text_color(rgb(self.palette.muted))
                .text_size(px(12.))
                .flex_shrink_0()
                .line_height(line_height)
                .left(self.cwd.display().to_string())
                .right(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .when_some(self.forward_status_button(cx), |row, button| {
                            row.child(button)
                        }),
                )
                .into_any_element(),
            Page::Explorer => {
                let entity = self.explorer_entity();
                let explorer = entity.as_ref().map(|entity| entity.read(cx));
                let cursor = explorer
                    .map(|explorer| explorer.file_editor.read(cx).cursor_position())
                    .unwrap_or_default();
                let path = explorer
                    .and_then(|explorer| explorer.open_file.as_ref())
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| self.cwd.display().to_string());
                StatusBar::new()
                    .h(height)
                    .min_h(height)
                    .py_0()
                    .px_3()
                    .bg(rgb(self.palette.panel))
                    .border_color(rgb(self.palette.border))
                    .text_color(rgb(self.palette.muted))
                    .text_size(px(12.))
                    .flex_shrink_0()
                    .line_height(line_height)
                    .left(div().min_w_0().flex_1().truncate().child(format!(
                        "{}{}",
                        path,
                        if explorer.is_some_and(|explorer| explorer.editor_dirty) {
                            " •"
                        } else {
                            ""
                        }
                    )))
                    .right(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                if explorer.is_some_and(|explorer| explorer.image_preview.is_some())
                                {
                                    "PNG".to_string()
                                } else if explorer.is_some_and(|explorer| explorer.preview_mode) {
                                    crate::t!("file.preview").to_string()
                                } else {
                                    format!(
                                        "{}  {}",
                                        explorer
                                            .map(|explorer| explorer.editor_language.as_str())
                                            .unwrap_or("plaintext"),
                                        crate::t!(
                                            "editor.ln_col",
                                            line = cursor.line + 1,
                                            column = cursor.character + 1
                                        )
                                    )
                                },
                            )
                            .when_some(self.forward_status_button(cx), |row, button| {
                                row.child(button)
                            }),
                    )
                    .into_any_element()
            }
            Page::Terminal => StatusBar::new()
                .h(height)
                .min_h(height)
                .py_0()
                .px(px(PANE_PADDING))
                .bg(rgba(0))
                .border_color(rgba(0))
                .flex_shrink_0()
                .text_size(px(12.))
                .line_height(line_height)
                .text_color(rgb(self.terminal_palette.muted))
                .left(self.terminal_status_strip(snapshot, metrics_config::Side::Left))
                .right(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(self.terminal_status_strip(snapshot, metrics_config::Side::Right))
                        .when_some(self.forward_status_button(cx), |row, button| {
                            row.child(button)
                        }),
                )
                .into_any_element(),
        }
    }
    fn new_session_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.editing_session.is_some() {
            return self.session_editor(cx);
        }
        let p = self.palette;
        let local = self.hosts[self.active].config.is_none();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab"
                    && !event.keystroke.modifiers.shift
                    && this.tab_path.read(cx).focus_handle(cx).is_focused(window)
                    && let Some(path) = this.directory_candidates.first().cloned()
                {
                    this.accept_directory(path, window, cx);
                    cx.stop_propagation();
                    window.prevent_default();
                }
            }))
            .child(
                Field::new()
                    .label(crate::t!("ws.name_optional").to_string())
                    .child(Input::new(&self.tab_name)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(11.))
                    .child(crate::t!("ws.directory"))
                    .child(
                        icon_button(
                            "session-directory-help",
                            IconName::Info,
                            crate::t!("ws.directory_hint"),
                        )
                        .size(px(20.))
                        .text_color(rgb(p.muted)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(!self.directory_candidates.is_empty(), |view| {
                                view.key_context("DirectoryCompletion")
                            })
                            .child(Input::new(&self.tab_path)),
                    )
                    .when(local, |row| {
                        row.child(
                            icon_button(
                                "browse-session-path",
                                IconName::FolderOpen,
                                crate::t!("ws.browse"),
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| this.browse_tab_directory(window, cx),
                            )),
                        )
                    })
                    .child(
                        Button::new("select-workspace-directory")
                            .small()
                            .label(crate::t!("ws.open"))
                            .disabled(self.session_directory(cx).is_none())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.create_tab(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("directory-completions")
                    .flex()
                    .flex_col()
                    .max_h(px(240.))
                    .overflow_y_scrollbar()
                    .children(
                        self.directory_candidates
                            .iter()
                            .enumerate()
                            .map(|(index, path)| {
                                let selected = path.clone();
                                let label = path
                                    .trim_end_matches(['/', '\\'])
                                    .rsplit(['/', '\\'])
                                    .next()
                                    .unwrap_or(path)
                                    .to_owned();
                                div()
                                    .id(("directory-candidate", index))
                                    .h(px(30.))
                                    .flex_shrink_0()
                                    .px_2()
                                    .rounded_sm()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .text_color(rgb(p.text))
                                    .hover(|style| style.bg(rgb(p.selected)))
                                    .child(
                                        Icon::new(IconName::Folder)
                                            .xsmall()
                                            .text_color(rgb(p.muted)),
                                    )
                                    .child(div().flex_1().min_w_0().truncate().child(label))
                                    .child(
                                        Icon::new(IconName::ChevronRight)
                                            .xsmall()
                                            .text_color(rgb(p.muted)),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.accept_directory(selected.clone(), window, cx)
                                    }))
                            }),
                    ),
            )
            .when_some(self.directory_error.clone(), |view, error| {
                view.child(
                    div()
                        .text_color(rgb(p.muted))
                        .text_size(px(11.))
                        .child(error),
                )
            })
            .when_some(self.message.clone(), |view, message| {
                view.child(
                    div()
                        .text_color(rgb(p.error))
                        .text_size(px(11.))
                        .child(message),
                )
            })
            .into_any_element()
    }
    fn host_validation_error(
        &mut self,
        field: HostField,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.host_error_field = Some(field);
        self.message = Some(message);
        let input = match field {
            HostField::Name => &self.label,
            HostField::Destination => &self.destination,
            HostField::User => &self.user,
            HostField::Port => &self.port,
            HostField::IdentityFile => &self.identity_file,
        };
        input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn host_field(&self, field: HostField, label: impl Into<SharedString>) -> Field {
        let error = (self.host_error_field == Some(field))
            .then(|| self.message.clone())
            .flatten();
        let color = self.palette.error;
        Field::new()
            .label(label.into())
            .when_some(error, |field, error| {
                field.description_fn(move |_, _| {
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(color))
                        .child(error.clone())
                })
            })
    }

    fn connection_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let name_only = self.editing_host == Some(0);
        let form = Form::vertical()
            .small()
            .columns(2)
            .child(
                self.host_field(HostField::Name, crate::t!("ws.field_name").into_owned())
                    .col_span(2)
                    .child(Input::new(&self.label)),
            )
            .child(
                self.host_field(
                    HostField::Destination,
                    crate::t!("ws.field_host").into_owned(),
                )
                .required(true)
                .visible(!name_only)
                .child(Input::new(&self.destination)),
            )
            .child(
                self.host_field(HostField::User, crate::t!("ws.field_user").into_owned())
                    .required(true)
                    .visible(!name_only)
                    .child(Input::new(&self.user)),
            )
            .child(
                self.host_field(HostField::Port, crate::t!("ws.field_port").into_owned())
                    .visible(!name_only)
                    .child(Input::new(&self.port)),
            );
        let authentication =
            Form::vertical().small().columns(2).child(
                self.host_field(
                    HostField::IdentityFile,
                    crate::t!("ws.field_identity_file").into_owned(),
                )
                .col_span(2)
                .visible(!name_only)
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(Input::new(&self.identity_file).flex_1())
                        .child(
                            icon_button(
                                "browse-identity",
                                IconName::FolderOpen,
                                crate::t!("ws.browse_identity_file"),
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| this.browse_identity_file(window, cx),
                            )),
                        ),
                ),
            );
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .id("host-form-content")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scrollbar()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(!name_only, |v| {
                        v.child(style::section_heading(
                            crate::t!("ws.connection_info").into_owned(),
                            p,
                        ))
                    })
                    .child(form)
                    .when(!name_only, |v| {
                        v.child(
                            style::section_heading(crate::t!("ssh.authentication").into_owned(), p)
                                .mt_2(),
                        )
                        .child(authentication)
                        .child(
                            Checkbox::new("use-tmux")
                                .label(crate::t!("ws.enable_tmux").into_owned())
                                .checked(self.use_tmux)
                                .on_click(cx.listener(|this, checked, _, cx| {
                                    this.use_tmux = *checked;
                                    cx.notify();
                                })),
                        )
                    })
                    .when_some(
                        self.message
                            .clone()
                            .filter(|_| self.host_error_field.is_none()),
                        |v, message| {
                            v.child(style::notice(IconName::CircleAlert, message, p.error))
                        },
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .flex_shrink_0()
                    .gap_3()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .child(
                        Button::new("cancel-editor")
                            .small()
                            .h(px(style::CONTROL_HEIGHT))
                            .label(crate::t!("ws.cancel"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel_host_editor(window, cx)
                            })),
                    )
                    .child(
                        button(
                            "save-editor",
                            IconName::Check,
                            if self.editing_host.is_some() {
                                crate::t!("ws.save")
                            } else {
                                crate::t!("ws.add_host")
                            },
                        )
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| this.add_host(window, cx))),
                    ),
            )
            .into_any_element()
    }
}
impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.keyboard_focus {
            Theme::global_mut(cx).ring = rgb(self.palette.accent).into();
        }
        self.subscribe_terminal_links(window, cx);
        for (index, host) in self.hosts.iter().enumerate() {
            let active = (index == self.active && self.page == Page::Terminal)
                .then(|| host.snapshot.window())
                .flatten();
            for (id, view) in &host.views {
                let visible =
                    active.is_some_and(|window| window.panes.iter().any(|pane| &pane.id == id));
                view.update(cx, |view, cx| view.set_visible(visible, cx));
            }
        }
        self.prepare_tools(window, cx);
        self.prepare_docker(window, cx);
        if self.need_focus
            && !window.has_active_dialog(cx)
            && !self.creating_tab
            && !self.settings
            && !self.command_palette
        {
            match self.page {
                Page::Explorer => {
                    if let Some(entity) = self.explorer_entity() {
                        entity.update(cx, |explorer, cx| explorer.focus(window, cx));
                    } else {
                        window.focus(&self.root_focus, cx);
                    }
                    self.need_focus = false;
                }
                Page::Git | Page::Docker => {
                    window.focus(&self.root_focus, cx);
                    self.need_focus = false;
                }
                Page::Terminal => {
                    if let Some(w) = self.hosts[self.active].snapshot.window() {
                        if let Some(v) = self.hosts[self.active].views.get(&w.active_pane) {
                            window.focus(&v.read(cx).focus_handle(cx), cx);
                            self.need_focus = false;
                        }
                    }
                }
            }
        }
        let p = self.palette;
        let snapshot = self.hosts[self.active].snapshot.clone();
        let current = snapshot.window();
        let message = self
            .message
            .clone()
            .filter(|_| !self.settings_ui.host_form)
            .or_else(|| snapshot.message.clone())
            .filter(|_| self.page != Page::Docker);
        let can_reconnect = snapshot.message.is_some()
            && self.hosts[self.active].config.is_some()
            && !self.settings_ui.host_form;
        let body = if self.page == Page::Terminal {
            let surface = self.surface(cx);
            let owner = cx.entity().downgrade();
            let close = owner.clone();
            div()
                .relative()
                .size_full()
                .child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .when(self.floating, |v| {
                            v.absolute()
                                .left(self.float_position.x)
                                .top(self.float_position.y)
                                .w(relative(0.85))
                                .h(relative(0.85))
                        })
                        .when(self.floating, |v| {
                            v.border_1().border_color(rgb(p.border)).shadow_lg().child(
                                div()
                                    .h(px(28.))
                                    .flex_shrink_0()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .bg(rgb(p.panel))
                                    .cursor(CursorStyle::ClosedHand)
                                    .child(
                                        div().flex_1().child(
                                            current.map(|w| w.name.clone()).unwrap_or_default(),
                                        ),
                                    )
                                    .child(
                                        icon_button(
                                            "dock-terminal",
                                            IconName::Minimize2,
                                            crate::t!("ws.restore_tiled"),
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.floating = false;
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, e: &MouseDownEvent, _, cx| {
                                            this.float_drag = Some(e.position);
                                            cx.notify();
                                        }),
                                    ),
                            )
                        })
                        .child(div().flex_1().min_h_0().child(surface)),
                )
                .context_menu(move |menu, _, _| {
                    let close = close.clone();
                    let close_window = owner.clone();
                    menu.item(PopupMenuItem::new(crate::t!("ws.close_pane")).on_click(
                        move |_, _, cx| {
                            let _ = close.update(cx, |this, cx| this.act(Action::ClosePane, cx));
                        },
                    ))
                    .item(
                        PopupMenuItem::new(crate::t!("ws.close_tab")).on_click(move |_, _, cx| {
                            let _ = close_window
                                .update(cx, |this, cx| this.act(Action::CloseWindow, cx));
                        }),
                    )
                })
                .into_any_element()
        } else {
            div().into_any_element()
        };
        let body = if self.page != Page::Terminal {
            div()
                .size_full()
                .bg(rgb(if self.page == Page::Git {
                    p.background
                } else {
                    self.terminal_palette.terminal
                }))
                .text_color(rgb(if self.page == Page::Git {
                    p.text
                } else {
                    self.terminal_palette.text
                }))
                .child(self.file_surface(cx))
                .into_any_element()
        } else {
            body
        };
        let content_background = rgba(
            (self.terminal_palette.terminal << 8) | (self.background_opacity * 255.).round() as u32,
        );
        let main = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(content_background)
            .when_some(message, |view, message| {
                view.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py_2()
                        .child(style::notice(IconName::CircleAlert, message, p.error).flex_1())
                        .when(can_reconnect, |view| {
                            view.child(
                                Button::new("reconnect-host")
                                    .small()
                                    .label(crate::t!("shortcut.reconnect"))
                                    .on_click(cx.listener(|this, _, _, cx| this.reconnect(cx))),
                            )
                        }),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_1().min_w_0().child(body)),
            )
            .when(!self.zen && self.show_status_bar, |view| {
                view.child(self.status_bar(&snapshot, window, cx))
            })
            .into_any_element();
        let sidebar_width = self.sidebar_width;
        let logo_accent = p.accent;
        let sidebar_background =
            rgba((p.panel << 8) | (self.sidebar_opacity * 255.).round() as u32);
        let workspace = if self.zen || self.sidebar_collapsed {
            main
        } else {
            div()
                .size_full()
                .flex()
                .child(
                    div()
                        .w(px(sidebar_width))
                        .flex_none()
                        .child(self.sidebar(cx)),
                )
                .child(div().flex_1().min_w_0().child(main))
                .into_any_element()
        };
        let title = if self.page == Page::Docker {
            self.docker.read(cx).status()
        } else {
            current
                .map(|tab| {
                    tab.panes
                        .iter()
                        .find(|pane| pane.id == tab.active_pane)
                        .map(|pane| pane.title.as_str())
                        .filter(|title| !title.is_empty())
                        .unwrap_or(&tab.name)
                })
                .unwrap_or_default()
                .to_owned()
        };
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let sheet_layer = Root::render_sheet_layer(window, cx);
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgba(0))
            .text_color(rgb(p.text))
            .font_family("Segoe UI")
            .text_size(px(13.))
            .line_height(relative(1.25))
            .track_focus(&self.root_focus)
            .capture_key_down(cx.listener(Self::keyboard))
            .child(
                canvas(|_, _, _| (), {
                    let owner = cx.entity().downgrade();
                    move |_, _, window, _| {
                        // Capture also covers controls that stop bubbling mouse events.
                        window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ =
                                    owner.update(cx, |app, cx| app.set_keyboard_focus(false, cx));
                            }
                        });
                    }
                })
                .absolute()
                .size_full(),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                this.move_transfer_panel(e, window, cx);
                if let Some((origin, width)) = this.sidebar_drag {
                    let max = (f32::from(window.viewport_size().width) - 320.).clamp(160., 520.);
                    this.sidebar_width =
                        (width + f32::from(e.position.x - origin)).clamp(160., max);
                    cx.notify();
                }
                if let Some(origin) = this.float_drag {
                    this.float_position.x = (this.float_position.x + e.position.x - origin.x)
                        .max(px(0.))
                        .min(px(150.));
                    this.float_position.y = (this.float_position.y + e.position.y - origin.y)
                        .max(px(0.))
                        .min(px(100.));
                    this.float_drag = Some(e.position);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.end_transfer_panel_drag(window, cx);
                    this.float_drag = None;
                    if this.sidebar_drag.take().is_some() {
                        this.save(cx);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.end_transfer_panel_drag(window, cx);
                    this.float_drag = None;
                    if this.sidebar_drag.take().is_some() {
                        this.save(cx);
                        cx.notify();
                    }
                }),
            )
            .when(!self.zen, |view| {
                view.child(
                    div()
                        .h(px(CHROME_BAR_HEIGHT))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .text_color(rgb(if self.page == Page::Terminal {
                            self.terminal_palette.muted
                        } else {
                            p.text
                        }))
                        .when(!self.sidebar_collapsed, |header| {
                            header.child(
                                div()
                                    .w(px(sidebar_width))
                                    .h_full()
                                    .flex_shrink_0()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .bg(sidebar_background)
                                    .text_color(rgb(p.text))
                                    .border_r_1()
                                    .border_color(rgb(p.border))
                                    .child(
                                        div()
                                            .h_full()
                                            .flex()
                                            .items_center()
                                            .window_control_area(WindowControlArea::Drag)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_size(px(16.))
                                            .line_height(chrome_line_height(window, 16.))
                                            .child("T")
                                            .child(div().text_color(rgb(logo_accent)).child("S"))
                                            .child("hell"),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .pl(px(WINDOW_CONTROLS_WIDTH))
                                .flex()
                                .items_center()
                                .justify_center()
                                .window_control_area(WindowControlArea::Drag)
                                .bg(if self.page == Page::Terminal {
                                    content_background
                                } else {
                                    rgb(p.panel).into()
                                })
                                .when(self.show_top_title, |view| {
                                    view.child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(12.))
                                            .line_height(chrome_line_height(window, 12.))
                                            .child(title),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .h_full()
                                .bg(if self.page == Page::Terminal {
                                    content_background
                                } else {
                                    rgb(p.panel).into()
                                })
                                .child(window_controls(window, self.terminal_palette.text)),
                        ),
                )
            })
            .child(div().flex_1().min_h_0().child(workspace))
            .when(!self.zen && !self.sidebar_collapsed, |view| {
                view.child(
                    div()
                        .id("sidebar-resize")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(sidebar_width - 3.))
                        .w(px(6.))
                        .cursor(CursorStyle::ResizeLeftRight)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |app, event: &MouseDownEvent, _, cx| {
                                app.sidebar_drag = Some((event.position.x, sidebar_width));
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                )
            })
            .when_some(self.transfer_panel(window, cx), |view, panel| {
                view.child(panel)
            })
            .children(dialog_layer)
            .children(sheet_layer)
    }
}
