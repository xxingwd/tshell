//! One workspace contract; local processes and tmux implement the same actions.
use crate::{terminal::Session, tmux_client::TmuxClient};
use anyhow::Result;
use std::sync::Arc;

mod layout;
mod local;

pub use local::LocalBackend;

pub const PANE_PADDING: f32 = 8.;
pub const PANE_GAP: f32 = 1.;

fn padded_cells(extent: f32, cell_size: f32, leading_padding: f32) -> usize {
    // Reserve the minimum inset on both sides; whole-cell rounding supplies the trailing remainder.
    ((extent - 2. * leading_padding).max(0.) / cell_size)
        .floor()
        .max(2.) as usize
}

#[derive(Clone, Debug)]
pub struct PaneBounds {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelViewport {
    pub width: f32,
    pub height: f32,
    pub cell_width: f32,
    pub line_height: f32,
    pub horizontal_padding: f32,
    pub vertical_padding: f32,
}

impl PixelViewport {
    pub fn terminal_size(self) -> (usize, usize) {
        (
            padded_cells(self.width, self.cell_width, self.horizontal_padding),
            padded_cells(self.height, self.line_height, self.vertical_padding),
        )
    }
}

impl PaneBounds {
    pub fn terminal_size(&self, viewport: PixelViewport) -> (usize, usize) {
        (
            padded_cells(self.width, viewport.cell_width, viewport.horizontal_padding),
            padded_cells(self.height, viewport.line_height, viewport.vertical_padding),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}
impl Direction {
    pub fn flag(self) -> &'static str {
        match self {
            Self::Left => "-L",
            Self::Right => "-R",
            Self::Up => "-U",
            Self::Down => "-D",
        }
    }
}

pub fn adjacent_pane(window: &WindowInfo, direction: Direction) -> Option<String> {
    let from = window.panes.iter().find(|p| p.id == window.active_pane)?;
    let center = |p: &PaneInfo| {
        (
            p.x as i64 * 2 + p.cols as i64,
            p.y as i64 * 2 + p.rows as i64,
        )
    };
    let (x, y) = center(from);
    window
        .panes
        .iter()
        .filter(|p| p.id != from.id)
        .filter_map(|p| {
            let (px, py) = center(p);
            let (forward, cross) = match direction {
                Direction::Left => (x - px, (y - py).abs()),
                Direction::Right => (px - x, (y - py).abs()),
                Direction::Up => (y - py, (x - px).abs()),
                Direction::Down => (py - y, (x - px).abs()),
            };
            (forward > 0).then_some((forward + cross * 3, p))
        })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, pane)| pane.id.clone())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneInfo {
    pub cwd: String,
    pub notice_count: u64,
    pub id: String,
    pub title: String,
    pub x: usize,
    pub y: usize,
    pub cols: usize,
    pub rows: usize,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: String,
    pub name: String,
    pub index: usize,
    pub cols: usize,
    pub rows: usize,
    pub active_pane: String,
    pub zoomed: bool,
    pub panes: Vec<PaneInfo>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub active_window: String,
    pub windows: Vec<WindowInfo>,
}
/// Client-owned session configuration; tmux sessions are never persisted here.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionProfile {
    pub name: String,
    pub path: String,
}
/// How far a host's connection has progressed.
///
/// Kept separate from [`Snapshot::message`], which carries error detail. Both
/// behaviour and wording derive from this state, so neither has to inspect
/// message text to find out what is happening.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    /// Still establishing; no terminal contents are available yet.
    #[default]
    Connecting,
    /// Usable. `message` may still carry a transient warning.
    Ready,
    /// Ended. `message` carries the reason when there is one.
    Closed,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub sessions: Vec<SessionInfo>,
    pub active_session: String,
    pub connection: Connection,
    pub message: Option<String>,
    pub revision: u64,
}
impl Snapshot {
    pub fn connected(&self) -> bool {
        self.connection == Connection::Ready
    }
    pub fn windows(&self) -> impl Iterator<Item = &WindowInfo> {
        self.sessions.iter().flat_map(|s| &s.windows)
    }
    pub fn windows_mut(&mut self) -> impl Iterator<Item = &mut WindowInfo> {
        self.sessions.iter_mut().flat_map(|s| &mut s.windows)
    }
    pub fn session_mut(&mut self) -> Option<&mut SessionInfo> {
        self.sessions
            .iter_mut()
            .find(|s| s.id == self.active_session)
    }

    pub fn session(&self) -> Option<&SessionInfo> {
        self.sessions.iter().find(|s| s.id == self.active_session)
    }
    pub fn window(&self) -> Option<&WindowInfo> {
        let s = self.session()?;
        s.windows.iter().find(|w| w.id == s.active_window)
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    RenameSession {
        id: String,
        name: String,
    },
    RenameWindow {
        id: String,
        name: String,
    },
    RemoveSession(String),
    NewWindowInSession(String),
    MoveFocus(Direction),
    MovePane(Direction),
    CycleLayout,
    SelectSession(String),
    SelectWindow(String),
    SelectPane(String),
    NewSession,
    NewSessionAt(String),
    NewNamedSession {
        path: String,
        name: String,
    },
    NewWindow,
    NewWindowAt {
        path: String,
        name: String,
    },
    Split(SplitAxis),
    ClosePane,
    CloseWindow,
    Zoom,
    ResizePane {
        id: String,
        axis: SplitAxis,
        amount: i32,
    },
}

pub enum Backend {
    Local(LocalBackend),
    Tmux(Arc<TmuxClient>),
}
impl Backend {
    pub fn listen(&self, tx: async_channel::Sender<()>) {
        match self {
            Self::Local(backend) => {
                for screen in backend.screens.values() {
                    screen.metadata.listen(tx.clone());
                }
            }
            Self::Tmux(backend) => backend.updates.listen(tx),
        }
    }
    pub fn set_theme(&self, theme: crate::terminal_protocol::TerminalTheme) {
        match self {
            Self::Local(b) => {
                for screen in b.screens.values() {
                    screen.set_theme(theme);
                }
            }
            Self::Tmux(b) => b.set_theme(theme),
        }
    }
    pub fn reap_finished(&mut self) {
        if let Self::Local(backend) = self {
            backend.reap_finished();
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        match self {
            Self::Local(b) => b.snapshot(),
            Self::Tmux(b) => b.snapshot(),
        }
    }
    pub fn screen(&self, id: &str) -> Option<Arc<Session>> {
        match self {
            Self::Local(b) => b.screens.get(id).cloned(),
            Self::Tmux(b) => b.screen(id),
        }
    }
    pub fn apply(&mut self, action: Action) -> Result<()> {
        match self {
            Self::Local(b) => b.apply(action),
            Self::Tmux(b) => b.apply(action),
        }
    }
    pub fn resize(&mut self, id: &str, cols: usize, rows: usize) {
        match self {
            Self::Local(b) => b.resize(id, cols, rows),
            Self::Tmux(b) => b.resize(id, cols, rows),
        }
    }
}
