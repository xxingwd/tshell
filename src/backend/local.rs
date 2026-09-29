//! Local and plain SSH workspace lifecycle and process IO.
use super::{
    Action, Connection, PaneBounds, PaneInfo, PixelViewport, SessionInfo, SessionProfile, Snapshot,
    SplitAxis, WindowInfo, adjacent_pane, layout::Layout,
};
use crate::terminal::Session;
use anyhow::{Result, bail};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

const MAX_LOCAL_PANES: usize = 12;

pub struct LocalBackend {
    _auxiliary: Option<crate::ssh_pool::Auxiliary>,
    state: Snapshot,
    layouts: BTreeMap<String, Layout>,
    pixel_viewports: BTreeMap<String, PixelViewport>,
    pub(super) screens: BTreeMap<String, Arc<Session>>,
    next: u64,
    cwd: PathBuf,
    ssh: Option<crate::tmux_client::HostConfig>,
}
impl LocalBackend {
    #[cfg(test)]
    pub fn new(cwd: PathBuf) -> Result<Self> {
        Self::with_transport(cwd, None)
    }
    #[cfg(test)]
    pub fn with_transport(
        cwd: PathBuf,
        ssh: Option<crate::tmux_client::HostConfig>,
    ) -> Result<Self> {
        Self::restore(cwd, ssh, None)
    }
    pub fn restore(
        cwd: PathBuf,
        ssh: Option<crate::tmux_client::HostConfig>,
        profiles: Option<Vec<SessionProfile>>,
    ) -> Result<Self> {
        let mut this = Self {
            _auxiliary: ssh.as_ref().map(crate::ssh_pool::auxiliary),
            state: Snapshot {
                connection: Connection::Ready,
                ..Default::default()
            },
            layouts: BTreeMap::new(),
            pixel_viewports: BTreeMap::new(),
            screens: BTreeMap::new(),
            next: 0,
            cwd,
            ssh,
        };
        match profiles {
            Some(profiles) => {
                for profile in profiles {
                    this.new_session(profile.name, profile.path)?;
                }
            }
            None => this.apply(Action::NewSession)?,
        }
        Ok(this)
    }
    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = self.state.clone();
        if self.ssh.is_some() && !self.screens.is_empty() {
            let mut connecting = false;
            let mut ready = false;
            for screen in self.screens.values() {
                if !screen.exited.load(std::sync::atomic::Ordering::Acquire) {
                    if screen.ready.load(std::sync::atomic::Ordering::Acquire) {
                        ready = true;
                    } else {
                        connecting = true;
                    }
                }
            }
            snapshot.connection = if ready {
                Connection::Ready
            } else if connecting {
                Connection::Connecting
            } else {
                Connection::Closed
            };
            if snapshot.connection == Connection::Closed {
                snapshot.message = self
                    .screens
                    .values()
                    .find_map(|screen| screen.error.lock().clone());
            }
        }
        snapshot
    }
    pub(super) fn reap_finished(&mut self) {
        let mut title_changed = false;
        for pane in self
            .state
            .windows_mut()
            .flat_map(|window| &mut window.panes)
        {
            let Some(screen) = self.screens.get(&pane.id) else {
                continue;
            };
            let cwd = screen.current_directory().unwrap_or_default();
            if pane.cwd != cwd {
                pane.cwd = cwd;
                title_changed = true;
            }
            let notices = screen
                .notice_count
                .load(std::sync::atomic::Ordering::Acquire);
            if pane.notice_count != notices {
                pane.notice_count = notices;
                title_changed = true;
            }
            let title = screen.title.lock();
            let fallback = crate::t!("term.shell");
            let title = if title.is_empty() {
                fallback.as_ref()
            } else {
                &title
            };
            if pane.title != title {
                pane.title = title.to_owned();
                title_changed = true;
            }
        }
        if title_changed {
            self.state.revision += 1;
        }
        let exited: Vec<_> = self
            .screens
            .iter()
            .filter(|(_, screen)| {
                screen.exited.load(std::sync::atomic::Ordering::Acquire)
                    && screen.error.lock().is_none()
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in exited {
            self.close_pane(&id);
        }
    }
    fn close_pane(&mut self, id: &str) {
        // Search the full layout: exited panes can be hidden by zoom or belong to background tabs.
        let window_id = self
            .layouts
            .iter()
            .find(|(_, layout)| layout.contains(id))
            .map(|(id, _)| id.clone());
        self.screens.remove(id);
        let Some(window_id) = window_id else {
            return;
        };
        let rest = self
            .layouts
            .remove(&window_id)
            .and_then(|layout| layout.without(id));
        if let Some(layout) = rest {
            self.layouts.insert(window_id.clone(), layout.normalized());
            if let Some(w) = self
                .state
                .sessions
                .iter_mut()
                .flat_map(|s| &mut s.windows)
                .find(|w| w.id == window_id)
            {
                if w.active_pane == id {
                    w.zoomed = false;
                }
                let (cols, rows) = (w.cols, w.rows);
                self.resize(&window_id, cols, rows);
            }
        } else {
            self.close_window(&window_id);
        }
        self.state.revision += 1;
    }
    fn remove_window_resources(&mut self, id: &str) {
        self.pixel_viewports.remove(id);
        if let Some(layout) = self.layouts.remove(id) {
            let mut panes = Vec::new();
            layout.pane_ids(&mut panes);
            for id in panes {
                self.screens.remove(&id);
            }
        }
    }
    fn close_window(&mut self, id: &str) {
        self.remove_window_resources(id);
        for session in &mut self.state.sessions {
            session.windows.retain(|w| w.id != id);
            if session.active_window == id {
                session.active_window = session
                    .windows
                    .first()
                    .map(|w| w.id.clone())
                    .unwrap_or_default();
            }
        }
        self.state.sessions.retain(|s| !s.windows.is_empty());
        if self.state.session().is_none() {
            self.state.active_session = self
                .state
                .sessions
                .first()
                .map(|s| s.id.clone())
                .unwrap_or_default();
        }
        self.state.revision += 1;
    }
    fn new_pane(&mut self, path: Option<String>) -> Result<String> {
        let id = self.id("local-pane-");
        let directory = path.as_deref();
        let session = if let Some(config) = &self.ssh {
            Session::ssh_config_in(config.clone(), directory)?
        } else {
            Arc::new(Session::start(
                directory
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.cwd.clone()),
            )?)
        };
        session.set_initial_directory(directory.map(str::to_owned).or_else(|| {
            self.ssh
                .is_none()
                .then(|| self.cwd.to_string_lossy().into_owned())
        }));
        self.screens.insert(id.clone(), session);
        Ok(id)
    }
    fn active_window_mut(&mut self) -> Option<&mut WindowInfo> {
        let session = self.state.session_mut()?;
        session
            .windows
            .iter_mut()
            .find(|w| w.id == session.active_window)
    }
    fn new_session(&mut self, name: String, path: String) -> Result<()> {
        anyhow::ensure!(
            !name.trim().is_empty() && !name.chars().any(char::is_control),
            "{}",
            crate::t!("local.session_name_invalid")
        );
        anyhow::ensure!(
            !path.chars().any(char::is_control),
            "{}",
            crate::t!("local.directory_invalid")
        );
        if self.ssh.is_none() {
            anyhow::ensure!(
                PathBuf::from(&path).is_dir(),
                "{}",
                crate::t!("local.directory_missing")
            );
        }
        // Start the process before mutating the workspace; failure leaves existing sessions intact.
        let pane = self.new_pane((!path.is_empty()).then_some(path.clone()))?;
        let id = self.id("local-session-");
        self.state.sessions.push(SessionInfo {
            id: id.clone(),
            name,
            cwd: path,
            ..Default::default()
        });
        self.state.active_session = id;
        self.add_window(pane);
        self.state.revision += 1;
        Ok(())
    }
    pub fn profiles(&self) -> Vec<SessionProfile> {
        self.state
            .sessions
            .iter()
            .map(|session| SessionProfile {
                name: session.name.clone(),
                path: session.cwd.clone(),
            })
            .collect()
    }
    fn new_window(&mut self, path: Option<String>) -> Result<()> {
        let session = self
            .state
            .session()
            .ok_or_else(|| anyhow::anyhow!("{}", crate::t!("local.no_session")))?;
        let path = path.or_else(|| (!session.cwd.is_empty()).then(|| session.cwd.clone()));
        let pane = self.new_pane(path)?;
        self.add_window(pane);
        Ok(())
    }
    fn add_window(&mut self, pane: String) {
        let id = self.id("local-window-");
        self.layouts.insert(id.clone(), Layout::Leaf(pane.clone()));
        let Some(s) = self.state.session_mut() else {
            return;
        };
        let index = s.windows.len();
        s.windows.push(WindowInfo {
            id: id.clone(),
            name: if cfg!(windows) {
                "PowerShell".to_string()
            } else {
                crate::t!("term.shell").to_string()
            },
            index,
            cols: 100,
            rows: 28,
            active_pane: pane,
            ..Default::default()
        });
        s.active_window = id.clone();
        self.resize(&id, 100, 28);
    }
    pub fn apply(&mut self, action: Action) -> Result<()> {
        match action {
            Action::NewSession => {
                let path = if self.ssh.is_some() {
                    String::new()
                } else {
                    self.cwd.to_string_lossy().into_owned()
                };
                self.new_session(crate::t!("local.default_session").to_string().into(), path)?;
            }
            Action::NewSessionAt(path) => {
                self.new_session(crate::t!("local.session").to_string(), path)?
            }
            Action::NewNamedSession { name, path } => self.new_session(name, path)?,
            Action::SelectSession(id) => {
                if self.state.sessions.iter().any(|s| s.id == id) {
                    self.state.active_session = id;
                }
            }
            Action::RenameSession { id, name } => {
                anyhow::ensure!(
                    !name.trim().is_empty() && !name.chars().any(char::is_control),
                    "{}",
                    crate::t!("local.session_name_invalid")
                );
                if let Some(session) = self.state.sessions.iter_mut().find(|s| s.id == id) {
                    session.name = name;
                }
            }
            Action::RemoveSession(id) => {
                let windows: Vec<_> = self
                    .state
                    .sessions
                    .iter()
                    .filter(|s| s.id == id)
                    .flat_map(|s| &s.windows)
                    .map(|w| w.id.clone())
                    .collect();
                for window in windows {
                    self.close_window(&window);
                }
            }
            Action::NewWindowInSession(id) => {
                self.apply(Action::SelectSession(id))?;
                self.new_window(None)?;
            }
            Action::RenameWindow { id, name } => {
                if let Some(window) = self
                    .state
                    .sessions
                    .iter_mut()
                    .flat_map(|s| &mut s.windows)
                    .find(|w| w.id == id)
                {
                    window.name = name;
                }
            }
            Action::MoveFocus(direction) | Action::MovePane(direction) => {
                if let Some(w) = self.state.window().cloned()
                    && let Some(target) = adjacent_pane(&w, direction)
                {
                    if matches!(action, Action::MovePane(_)) {
                        if let Some(layout) = self.layouts.get_mut(&w.id) {
                            layout.swap(&w.active_pane, &target);
                        }
                        self.resize(&w.id, w.cols, w.rows);
                    } else {
                        self.apply(Action::SelectPane(target))?;
                    }
                }
            }
            Action::CycleLayout => {
                if let Some(w) = self.state.window().cloned() {
                    if let Some(layout) = self.layouts.get_mut(&w.id) {
                        layout.cycle();
                    }
                    self.resize(&w.id, w.cols, w.rows);
                }
            }
            Action::NewWindow => self.new_window(None)?,
            Action::NewWindowAt { path, name } => {
                anyhow::ensure!(
                    !path.is_empty() && !path.chars().any(char::is_control),
                    "{}",
                    crate::t!("local.directory_invalid")
                );
                if self.ssh.is_none() {
                    anyhow::ensure!(
                        PathBuf::from(&path).is_dir(),
                        "{}",
                        crate::t!("local.directory_missing")
                    );
                }
                self.new_window(Some(path))?;
                if !name.is_empty() {
                    let id = self.state.window().unwrap().id.clone();
                    if let Some(w) = self
                        .state
                        .sessions
                        .iter_mut()
                        .flat_map(|s| &mut s.windows)
                        .find(|w| w.id == id)
                    {
                        w.name = name;
                    }
                }
            }
            Action::SelectWindow(id) => {
                if let Some(session) = self
                    .state
                    .sessions
                    .iter_mut()
                    .find(|s| s.windows.iter().any(|w| w.id == id))
                {
                    session.active_window = id;
                    self.state.active_session = session.id.clone();
                }
            }
            Action::SelectPane(id) => {
                if let Some(w) = self.active_window_mut()
                    && w.panes.iter().any(|p| p.id == id)
                {
                    w.active_pane = id;
                }
            }
            Action::Split(axis) => {
                let Some(window) = self.state.window().cloned() else {
                    bail!("{}", crate::t!("local.no_window"))
                };
                if self
                    .layouts
                    .get(&window.id)
                    .is_some_and(|layout| layout.pane_count() >= MAX_LOCAL_PANES)
                {
                    bail!("{}", crate::t!("local.pane_limit", max = MAX_LOCAL_PANES));
                }
                let path = self
                    .screens
                    .get(&window.active_pane)
                    .and_then(|screen| screen.current_directory());
                let pane = self.new_pane(path)?;
                if let Some(layout) = self.layouts.get_mut(&window.id) {
                    match axis {
                        SplitAxis::Horizontal => layout.add_column(&pane),
                        SplitAxis::Vertical => {
                            layout.add_row(&window.active_pane, &pane);
                        }
                    }
                }
                if let Some(w) = self.active_window_mut() {
                    w.active_pane = pane;
                    w.zoomed = false;
                }
                self.resize(&window.id, window.cols, window.rows);
            }
            Action::Zoom => {
                if let Some(w) = self.active_window_mut() {
                    w.zoomed = !w.zoomed;
                    let (id, c, r) = (w.id.clone(), w.cols, w.rows);
                    self.resize(&id, c, r);
                }
            }
            Action::ClosePane => {
                if let Some(window) = self.state.window().cloned() {
                    self.close_pane(&window.active_pane);
                }
            }
            Action::CloseWindow => {
                if let Some(window) = self.state.window().cloned() {
                    self.close_window(&window.id);
                }
            }
            Action::ResizePane { id, axis, amount } => {
                if let Some(w) = self.state.window().cloned() {
                    if let Some(layout) = self.layouts.get_mut(&w.id) {
                        layout.adjust(&id, axis, amount, w.cols, w.rows);
                    }
                    self.resize(&w.id, w.cols, w.rows);
                }
            }
        }
        self.state.revision += 1;
        Ok(())
    }
    pub fn pixel_panes(&self, window: &WindowInfo, viewport: PixelViewport) -> Vec<PaneBounds> {
        window_pixel_panes(self.layouts.get(&window.id), window, viewport)
    }
    pub fn resize_pixels(&mut self, id: &str, viewport: PixelViewport) {
        self.pixel_viewports.insert(id.into(), viewport);
        self.resize(
            id,
            (viewport.width / viewport.cell_width) as usize,
            (viewport.height / viewport.line_height) as usize,
        );
    }

    pub fn resize(&mut self, id: &str, cols: usize, rows: usize) {
        let cols = cols.clamp(12, 500);
        let rows = rows.clamp(6, 200);
        let Some(w) = self
            .state
            .sessions
            .iter_mut()
            .flat_map(|s| &mut s.windows)
            .find(|w| w.id == id)
        else {
            return;
        };
        w.cols = cols;
        w.rows = rows;
        let mut panes = Vec::new();
        if w.zoomed {
            panes.push(PaneInfo {
                id: w.active_pane.clone(),
                cols,
                rows,
                title: crate::t!("term.shell").to_string(),
                ..Default::default()
            });
        } else if let Some(layout) = self.layouts.get(id) {
            layout.panes(0, 0, cols, rows, &mut panes);
        }
        if !panes.iter().any(|p| p.id == w.active_pane) {
            w.active_pane = panes.first().map(|p| p.id.clone()).unwrap_or_default();
        }
        let pixel_sizes: BTreeMap<_, _> = self
            .pixel_viewports
            .get(id)
            .map(|viewport| {
                window_pixel_panes(self.layouts.get(id), w, *viewport)
                    .into_iter()
                    .map(|p| (p.id.clone(), p.terminal_size(*viewport)))
                    .collect()
            })
            .unwrap_or_default();
        for p in &mut panes {
            if let Some(screen) = self.screens.get(&p.id) {
                let (cols, rows) = pixel_sizes.get(&p.id).copied().unwrap_or((p.cols, p.rows));
                p.cwd = screen.current_directory().unwrap_or_default();
                screen.resize(rows, cols);
            }
        }
        w.panes = panes;
        self.state.revision += 1;
    }
}

fn window_pixel_panes(
    layout: Option<&Layout>,
    window: &WindowInfo,
    viewport: PixelViewport,
) -> Vec<PaneBounds> {
    let bounds = PaneBounds {
        id: window.active_pane.clone(),
        x: 0.,
        y: 0.,
        width: viewport.width,
        height: viewport.height,
    };
    if window.zoomed {
        return vec![bounds];
    }
    let mut panes = Vec::new();
    if let Some(layout) = layout {
        layout.pixel_panes(bounds, &mut panes);
    }
    panes
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ssh_connection_tracks_pty_readiness_and_failure() {
        use std::sync::atomic::Ordering;

        let mut backend =
            LocalBackend::restore(std::env::temp_dir(), None, Some(Vec::new())).unwrap();
        backend.ssh = Some(crate::tmux_client::HostConfig {
            destination: "example.invalid".into(),
            name: String::new(),
            user: "test".into(),
            port: None,
            identity_file: None,
            tmux: false,
            socket: None,
        });
        let screen = Session::remote("test".into(), 28, 100, Arc::new(|_| Ok(())));
        screen.ready.store(false, Ordering::Release);
        backend.screens.insert("pane".into(), screen.clone());
        assert_eq!(backend.snapshot().connection, Connection::Connecting);
        screen.ready.store(true, Ordering::Release);
        assert_eq!(backend.snapshot().connection, Connection::Ready);
        *screen.error.lock() = Some("Connection failed".into());
        screen.exited.store(true, Ordering::Release);
        let snapshot = backend.snapshot();
        assert_eq!(snapshot.connection, Connection::Closed);
        assert_eq!(snapshot.message.as_deref(), Some("Connection failed"));

        let live = Session::remote("live".into(), 28, 100, Arc::new(|_| Ok(())));
        backend.screens.insert("live".into(), live);
        assert_eq!(backend.snapshot().connection, Connection::Ready);
    }
    use crate::backend::Direction;

    fn check_background_output(mut backend: LocalBackend, remote: bool) {
        let first = backend.snapshot().window().unwrap().clone();
        backend.apply(Action::NewWindow).unwrap();
        let selected = backend.snapshot().window().unwrap().id.clone();
        let command = if cfg!(windows) && !remote {
            "Write-Output ('background-' + 'alive')\r"
        } else {
            "printf '%s%s\\n' 'background-' 'alive'\r"
        };
        let screen = &backend.screens[&first.active_pane];
        screen.input(command.as_bytes().to_vec()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let text: String = screen
                .term
                .lock()
                .grid()
                .display_iter()
                .map(|cell| cell.c)
                .collect();
            if text.contains("background-alive") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background output missing: {text}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        backend.reap_finished();
        let snapshot = backend.snapshot();
        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.windows().count(), 2);
        assert_eq!(snapshot.window().unwrap().id, selected);
    }

    #[test]
    fn local_background_terminals_keep_reading() {
        check_background_output(
            LocalBackend::new(std::env::current_dir().unwrap()).unwrap(),
            false,
        );
    }

    #[test]
    #[ignore = "requires TSHELL_SSH_TEST_HOST; verifies background plain SSH terminals"]
    fn plain_ssh_background_terminals_keep_reading() {
        let config = crate::tmux_client::HostConfig {
            destination: std::env::var("TSHELL_SSH_TEST_HOST").unwrap(),
            name: String::new(),
            user: String::new(),
            port: None,
            identity_file: None,
            tmux: false,
            socket: None,
        };
        check_background_output(
            LocalBackend::with_transport(std::env::current_dir().unwrap(), Some(config)).unwrap(),
            true,
        );
    }

    #[test]
    #[cfg(windows)]
    fn split_inherits_live_powershell_directory() {
        let mut backend = LocalBackend::new(std::env::current_dir().unwrap()).unwrap();
        let path = std::env::temp_dir();
        let pane = backend.snapshot().window().unwrap().active_pane.clone();
        backend.screens[&pane]
            .input(
                format!(
                    "Set-Location -LiteralPath '{}'\r",
                    path.display().to_string().replace('\'', "''")
                )
                .into_bytes(),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let directory = backend.screens[&pane]
                .current_directory()
                .map(PathBuf::from);
            if directory
                .as_ref()
                .and_then(|p| p.canonicalize().ok())
                .as_ref()
                == Some(&path.canonicalize().unwrap())
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "directory not reported: {directory:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        backend.apply(Action::Split(SplitAxis::Horizontal)).unwrap();
        let active = backend.snapshot().window().unwrap().active_pane.clone();
        assert_eq!(
            backend.screens[&active]
                .current_directory()
                .map(PathBuf::from)
                .and_then(|path| path.canonicalize().ok()),
            Some(path.canonicalize().unwrap())
        );
    }

    #[test]
    fn pixel_viewport_resizes_pty_after_split_zoom_and_window_resize() {
        use alacritty_terminal::grid::Dimensions;
        let mut backend = LocalBackend::new(std::env::current_dir().unwrap()).unwrap();
        let id = backend.snapshot().window().unwrap().id.clone();
        let mut viewport = PixelViewport {
            width: 1001.,
            height: 602.,
            cell_width: 8.,
            line_height: 21.,
            horizontal_padding: super::super::PANE_PADDING,
            vertical_padding: super::super::PANE_PADDING,
        };
        backend.resize_pixels(&id, viewport);
        backend.apply(Action::Split(SplitAxis::Horizontal)).unwrap();
        let check = |backend: &LocalBackend, viewport| {
            let window = backend.snapshot().window().unwrap().clone();
            for pane in backend.pixel_panes(&window, viewport) {
                let term = backend.screens[&pane.id].term.lock();
                assert_eq!(
                    (term.columns(), term.screen_lines()),
                    pane.terminal_size(viewport)
                );
            }
        };
        check(&backend, viewport);
        backend.apply(Action::Zoom).unwrap();
        check(&backend, viewport);
        viewport.width = 1373.;
        viewport.height = 701.;
        backend.resize_pixels(&id, viewport);
        check(&backend, viewport);
        backend.apply(Action::Zoom).unwrap();
        check(&backend, viewport);
        backend.close_window(&id);
        assert!(!backend.pixel_viewports.contains_key(&id));
    }

    #[test]
    fn exiting_shells_close_hidden_panes_background_tabs_and_last_tab() {
        let mut backend = LocalBackend::new(std::env::current_dir().unwrap()).unwrap();
        let first_window = backend.snapshot().window().unwrap().id.clone();
        let hidden = backend.snapshot().window().unwrap().active_pane.clone();
        backend.apply(Action::Split(SplitAxis::Horizontal)).unwrap();
        let focused = backend.snapshot().window().unwrap().active_pane.clone();
        backend.apply(Action::Zoom).unwrap();
        let exit = |backend: &LocalBackend, id: &str| {
            let screen = backend.screens.get(id).unwrap();
            screen.input(b"exit 7\r".to_vec()).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !screen.exited.load(std::sync::atomic::Ordering::Acquire) {
                assert!(std::time::Instant::now() < deadline, "shell did not exit");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        // A hidden pane exits without changing the focused pane or its zoom state.
        exit(&backend, &hidden);
        backend.reap_finished();
        assert!(!backend.screens.contains_key(&hidden));
        assert_eq!(backend.snapshot().window().unwrap().active_pane, focused);
        assert!(backend.snapshot().window().unwrap().zoomed);
        backend.apply(Action::Zoom).unwrap();
        assert_eq!(backend.snapshot().window().unwrap().panes.len(), 1);

        backend.apply(Action::NewWindow).unwrap();
        let second = backend.snapshot().window().unwrap().clone();
        exit(&backend, &focused);
        backend.reap_finished();
        assert_eq!(backend.snapshot().window().unwrap().id, second.id);
        assert!(!backend.layouts.contains_key(&first_window));
        assert_eq!(backend.snapshot().windows().count(), 1);

        exit(&backend, &second.active_pane);
        backend.reap_finished();
        assert!(backend.snapshot().sessions.is_empty());
        assert!(backend.screens.is_empty());
        assert!(backend.layouts.is_empty());
        let revision = backend.snapshot().revision;
        backend.reap_finished();
        assert_eq!(backend.snapshot().revision, revision);
    }

    #[test]
    fn ordinary_terminals_use_sessions_and_keep_window_operations() {
        let mut b = LocalBackend::new(std::env::current_dir().unwrap()).unwrap();
        assert_eq!(b.snapshot().sessions.len(), 1);
        b.apply(Action::Split(SplitAxis::Horizontal)).unwrap();
        let right = b.snapshot().window().unwrap().active_pane.clone();
        b.apply(Action::MoveFocus(Direction::Left)).unwrap();
        assert_ne!(b.snapshot().window().unwrap().active_pane, right);
        b.apply(Action::MoveFocus(Direction::Right)).unwrap();
        assert_eq!(b.snapshot().window().unwrap().active_pane, right);
        b.apply(Action::MovePane(Direction::Left)).unwrap();
        assert_eq!(b.snapshot().window().unwrap().panes[0].id, right);
        b.apply(Action::CycleLayout).unwrap();
        assert!(b.snapshot().window().unwrap().panes.iter().any(|p| p.y > 0));
        b.apply(Action::Split(SplitAxis::Vertical)).unwrap();
        let w = b.snapshot().window().unwrap().clone();
        assert_eq!(w.panes.len(), 3);
        b.apply(Action::Zoom).unwrap();
        assert_eq!(b.snapshot().window().unwrap().panes.len(), 1);
        b.apply(Action::Zoom).unwrap();
        assert_eq!(b.snapshot().window().unwrap().panes.len(), 3);
        b.apply(Action::ClosePane).unwrap();
        assert_eq!(b.snapshot().window().unwrap().panes.len(), 2);
        b.apply(Action::NewWindow).unwrap();
        assert_eq!(b.snapshot().windows().count(), 2);
        let background = b.snapshot().windows().next().unwrap().id.clone();
        b.apply(Action::SelectWindow(background)).unwrap();
        assert_eq!(b.snapshot().window().unwrap().panes.len(), 2);
        b.apply(Action::CloseWindow).unwrap();
        assert_eq!(b.snapshot().windows().count(), 1);
        assert_eq!(b.screens.len(), 1);
    }

    #[test]
    fn zoom_does_not_bypass_the_pane_limit() {
        let mut backend = LocalBackend::new(std::env::current_dir().unwrap()).unwrap();
        let window = backend.snapshot().window().unwrap().clone();
        let mut layout = Layout::Leaf(window.active_pane);
        for index in 1..MAX_LOCAL_PANES {
            layout = Layout::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.5,
                first: Box::new(layout),
                second: Box::new(Layout::Leaf(format!("pane-{index}"))),
            };
        }
        backend.layouts.insert(window.id, layout);
        backend.apply(Action::Zoom).unwrap();
        assert_eq!(backend.snapshot().window().unwrap().panes.len(), 1);
        assert!(backend.apply(Action::Split(SplitAxis::Horizontal)).is_err());
    }

    #[test]
    #[ignore = "requires TSHELL_SSH_TEST_HOST; plain SSH session directories and live background terminals"]
    fn plain_ssh_sessions_keep_separate_directories() {
        let config = crate::tmux_client::HostConfig {
            destination: std::env::var("TSHELL_SSH_TEST_HOST").unwrap(),
            name: String::new(),
            user: String::new(),
            port: None,
            identity_file: None,
            tmux: false,
            socket: None,
        };
        let profiles = vec![
            SessionProfile {
                name: "tmp".into(),
                path: "/tmp".into(),
            },
            SessionProfile {
                name: "root".into(),
                path: "/".into(),
            },
        ];
        let mut backend = LocalBackend::restore(
            std::env::current_dir().unwrap(),
            Some(config),
            Some(profiles.clone()),
        )
        .unwrap();
        let initial = backend.snapshot();
        let first = initial.sessions[0].id.clone();
        let screens: Vec<_> = initial
            .sessions
            .iter()
            .map(|session| {
                (
                    session.cwd.clone(),
                    backend.screens[&session.windows[0].active_pane].clone(),
                )
            })
            .collect();
        for (_, screen) in &screens {
            screen
                .input(b"printf 'profile-path:%s\\n' \"$PWD\"\r".to_vec())
                .unwrap();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        for (path, screen) in screens {
            loop {
                let text: String = screen
                    .term
                    .lock()
                    .grid()
                    .display_iter()
                    .map(|cell| cell.c)
                    .collect();
                if text.contains(&format!("profile-path:{path}")) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "remote session directory mismatch: {text}; {:?}",
                    screen.error.lock()
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        backend.apply(Action::SelectSession(first)).unwrap();
        backend.apply(Action::NewWindow).unwrap();
        assert_eq!(backend.snapshot().window().unwrap().panes[0].cwd, "/tmp");
        assert_eq!(backend.profiles(), profiles);
    }

    #[test]
    fn session_profiles_restore_names_and_directories_without_layouts() {
        let root = std::env::current_dir().unwrap();
        let mut backend = LocalBackend::restore(root.clone(), None, Some(Vec::new())).unwrap();
        backend
            .apply(Action::NewNamedSession {
                name: "first".into(),
                path: root.to_string_lossy().into_owned(),
            })
            .unwrap();
        let first = backend.snapshot().active_session;
        let first_pane = backend.snapshot().window().unwrap().active_pane.clone();
        let screen = backend.screens[&first_pane].clone();
        backend.apply(Action::NewWindow).unwrap();
        backend
            .apply(Action::NewNamedSession {
                name: "second".into(),
                path: root.join("src").to_string_lossy().into_owned(),
            })
            .unwrap();
        let second = backend.snapshot().active_session;
        assert_eq!(backend.snapshot().sessions.len(), 2);
        backend.apply(Action::SelectSession(first.clone())).unwrap();
        assert_eq!(backend.snapshot().session().unwrap().windows.len(), 2);
        assert!(Arc::ptr_eq(&screen, &backend.screens[&first_pane]));
        backend
            .apply(Action::RenameSession {
                id: first,
                name: "renamed".into(),
            })
            .unwrap();
        let saved = backend.profiles();
        let encoded = serde_json::to_string(&saved).unwrap();
        let json: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert!(
            json.as_array().unwrap().iter().all(|profile| {
                profile.get("windows").is_none() && profile.get("panes").is_none()
            })
        );
        let restored = LocalBackend::restore(
            root.clone(),
            None,
            Some(serde_json::from_str(&encoded).unwrap()),
        )
        .unwrap();
        assert_eq!(restored.profiles(), saved);
        assert!(
            restored
                .snapshot()
                .sessions
                .iter()
                .all(|s| s.windows.len() == 1)
        );
        backend.apply(Action::RemoveSession(second)).unwrap();
        assert_eq!(backend.profiles().len(), 1);
        assert_eq!(backend.profiles()[0].name, "renamed");
        let empty = LocalBackend::restore(root, None, Some(Vec::new())).unwrap();
        assert!(empty.snapshot().sessions.is_empty());
    }

    #[test]
    fn local_snapshot_uses_osc_title_for_its_shell_name() {
        let mut backend = LocalBackend::new(std::env::current_dir().unwrap()).unwrap();
        let pane = backend.snapshot().window().unwrap().active_pane.clone();
        backend
            .screens
            .get(&pane)
            .unwrap()
            .remote_restore(b"\x1b]2;Development server\x07");
        backend.reap_finished();
        assert_eq!(
            backend.snapshot().window().unwrap().panes[0].title,
            "Development server"
        );
    }
}
