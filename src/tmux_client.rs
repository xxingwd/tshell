//! One dedicated SSH connection/control channel per tmux session; host tools use auxiliary SSH.
//! tmux owns sessions, windows and panes; the client only keeps live state.
mod streams;
use crate::{
    backend::{Action, Connection, SessionInfo, Snapshot, SplitAxis, WindowInfo},
    terminal::{OutputWake, Session, validate_destination},
    terminal_protocol::{TerminalTheme, default_theme},
    tmux::{ControlEvent, ControlParser, parse_layout},
};
use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{self, Read, Write},
    process::Command,
    sync::{
        Arc,
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostConfig {
    pub destination: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub user: String,
    pub port: Option<u16>,
    #[serde(default)]
    pub identity_file: Option<std::path::PathBuf>,
    #[serde(default = "default_tmux")]
    pub tmux: bool,
    #[serde(skip)]
    pub socket: Option<String>,
}
fn default_tmux() -> bool {
    true
}
impl HostConfig {
    pub(crate) fn ssh_command(&self) -> Result<Command> {
        self.ssh_transport()
    }
    fn ssh_transport(&self) -> Result<Command> {
        validate_destination(&self.destination)?;
        let mut command = Command::new("ssh");
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
        ]);
        if let Some(port) = self.port {
            command.args(["-p", &port.to_string()]);
        }
        if let Some(identity_file) = &self.identity_file {
            command.arg("-i").arg(identity_file);
        }
        command.arg("--").arg(&self.destination);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        Ok(command)
    }
    pub(crate) fn tmux(&self) -> Result<String> {
        if let Some(socket) = &self.socket {
            anyhow::ensure!(
                socket
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "invalid socket name"
            );
            Ok(format!("tmux -u -L {socket}"))
        } else {
            Ok("tmux -u".into())
        }
    }
}

struct State {
    updates: OutputWake,
    published_revision: u64,
    theme: TerminalTheme,
    snapshot: Snapshot,
    screens: BTreeMap<String, Arc<Session>>,
}
enum Message {
    Wake,
    StreamOpened(String, Result<streams::Stream, String>),
    StreamEvent(String, ControlEvent),
    StreamClosed(String, Option<String>),
    Reply(Response, bool, Vec<Vec<u8>>),
    Probe(async_channel::Sender<Result<Duration>>),
    Discovery(Result<Draft>),
    Action(Action),
    Input(String, Vec<u8>),
    Resize(String, usize, usize),
    Stop,
}
enum Response {
    Ignore,
    Probe(Instant, async_channel::Sender<Result<Duration>>),
    Relayout,
    Sessions,
    Windows,
    Panes,
    Current,
    Capture {
        id: String,
        alternate: bool,
        cols: usize,
        rows: usize,
    },
    Modes(String),
    CreatedSession,
}

fn restored_terminal_modes(line: &[u8]) -> Option<Vec<u8>> {
    let line = String::from_utf8_lossy(line);
    let fields: Vec<_> = line.split('\t').collect();
    if fields.len() != 11 {
        return None;
    }
    let x = fields[0].parse::<usize>().unwrap_or(0);
    let y = fields[1].parse::<usize>().unwrap_or(0);
    let enabled = |field: usize| if fields[field] == "1" { 'h' } else { 'l' };
    let private_modes = fields[10].split(',').any(|mode| mode == "1004");
    Some(
        format!(
            "\x1b[{};{}H\x1b[?25{}\x1b[?1{}\x1b[?2004{}\x1b[?1000{}\x1b[?1002{}\x1b[?1003{}\x1b[?1005{}\x1b[?1006{}\x1b[?1004{}",
            y + 1,
            x + 1,
            enabled(2),
            enabled(3),
            enabled(4),
            enabled(5),
            enabled(6),
            enabled(7),
            enabled(8),
            enabled(9),
            if private_modes { 'h' } else { 'l' },
        )
        .into_bytes(),
    )
}
struct Request {
    command: String,
    target: Option<String>,
    response: Response,
    leading_ignores: usize,
    directory: Option<String>,
}
impl Request {
    fn session(mut self, id: Option<&String>) -> Self {
        self.target = id.cloned();
        self
    }
}
#[derive(Default)]
struct Draft {
    sessions: Vec<Vec<u8>>,
    windows: Vec<Vec<u8>>,
    panes: Vec<Vec<u8>>,
}

pub struct TmuxClient {
    pub updates: OutputWake,
    shared: Arc<Mutex<State>>,
    tx: SyncSender<Message>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
}
impl TmuxClient {
    pub fn connect(config: HostConfig) -> Arc<Self> {
        let updates = OutputWake::default();
        let shared = Arc::new(Mutex::new(State {
            updates: updates.clone(),
            published_revision: 0,
            theme: default_theme(),
            snapshot: Snapshot {
                connection: Connection::Connecting,
                ..Default::default()
            },
            screens: BTreeMap::new(),
        }));
        let (tx, rx) = mpsc::sync_channel(256);
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let client = Arc::new(Self {
            updates,
            shared: shared.clone(),
            tx: tx.clone(),
            stopped: stopped.clone(),
        });
        thread::spawn(move || {
            if let Err(error) = run(config, shared.clone(), tx, rx, stopped) {
                let mut s = shared.lock();
                s.snapshot.connection = Connection::Closed;
                s.snapshot.message = Some(format!("{error:#}"));
                s.snapshot.revision += 1;
                s.updates.notify();
            }
        });
        client
    }
    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().snapshot.clone()
    }
    pub fn screen(&self, id: &str) -> Option<Arc<Session>> {
        self.shared.lock().screens.get(id).cloned()
    }
    pub fn set_theme(&self, theme: TerminalTheme) {
        self.shared.lock().theme = theme;
        let _ = self.tx.try_send(Message::Wake);
        // The control loop reports colours before sending subscribed pane updates.
    }
    pub fn apply(&self, action: Action) -> Result<()> {
        self.tx
            .try_send(Message::Action(action))
            .context(crate::t!("tmux.apply_closed"))
    }
    pub fn resize(&self, id: &str, cols: usize, rows: usize) {
        let _ = self.tx.try_send(Message::Resize(
            id.into(),
            cols.clamp(12, 500),
            rows.clamp(6, 200),
        ));
    }

    /// Measure a lightweight tmux control-command round trip.
    pub fn probe(&self) -> Result<async_channel::Receiver<Result<Duration>>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.tx
            .try_send(Message::Probe(sender))
            .context(crate::t!("tmux.apply_closed"))?;
        Ok(receiver)
    }
}
impl Drop for TmuxClient {
    fn drop(&mut self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = self.tx.try_send(Message::Stop);
        // Disconnecting never sends kill-session / kill-server to tmux.
    }
}

fn q(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "'\\''"))
}
fn shell_q(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn shell_directory(path: &str) -> String {
    if path == "~" {
        "\"$HOME\"".into()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("\"$HOME\"/{}", shell_q(rest))
    } else {
        shell_q(path)
    }
}

fn request(command: impl Into<String>, response: Response) -> Request {
    Request {
        command: command.into(),
        target: None,
        directory: None,
        response,
        leading_ignores: 0,
    }
}
fn capture_request(id: &str, alternate: bool, cols: usize, rows: usize) -> Request {
    request(
        format!("capture-pane -p -e -N -t {} -S -2000", q(id)),
        Response::Capture {
            id: id.to_owned(),
            alternate,
            cols,
            rows,
        },
    )
}
fn capture_matches_size(screen: &Session, cols: usize, rows: usize) -> bool {
    use alacritty_terminal::grid::Dimensions;

    let term = screen.term.lock();
    term.columns() == cols && term.screen_lines() == rows
}
fn should_capture_pane(fresh: bool, changed_window: bool, pending_recapture: bool) -> bool {
    fresh || changed_window || pending_recapture
}
fn request_sequence(commands: Vec<String>, response: Response) -> Request {
    debug_assert!(!commands.is_empty());
    Request {
        leading_ignores: commands.len().saturating_sub(1),
        command: commands.join(" ; "),
        target: None,
        directory: None,
        response,
    }
}
fn is_pane_exit_notification(line: &str) -> bool {
    line.starts_with("%pane-exited ") || line.starts_with("%pane-died ")
}
fn refresh(queue: &mut VecDeque<Request>) {
    queue.push_back(request(
        "list-sessions -F '#{session_id}\t#{session_name}\t#{session_path}'",
        Response::Sessions,
    ));
    queue.push_back(request("list-windows -a -F '#{session_id}\t#{window_id}\t#{window_index}\t#{window_active}\t#{window_width}\t#{window_height}\t#{window_visible_layout}\t#{window_name}'",Response::Windows));
    queue.push_back(request("list-panes -a -F '#{pane_id}\t#{window_id}\t#{pane_active}\t#{alternate_on}\t#{pane_current_command}\t#{pane_title}\t#{pane_current_path}'",Response::Panes));
    queue.push_back(request(
        "display-message -p '#{session_id}'",
        Response::Current,
    ));
}

fn discover(config: &HostConfig, initialize: bool) -> Result<Draft> {
    let tmux = config.tmux()?;
    let ensure = if initialize {
        format!(
            "if ! {tmux} has-session 2>/dev/null; then {tmux} new-session -d -s tshell || exit; fi; "
        )
    } else {
        String::new()
    };
    let command = format!(
        "{ensure}if ! {tmux} has-session 2>/dev/null; then exit 0; fi; {tmux} -C list-sessions -F 'Sx#{{q:session_id}} x#{{q:session_name}} x#{{q:session_path}}'; {tmux} -C list-windows -a -F 'Wx#{{q:session_id}} x#{{q:window_id}} x#{{q:window_index}} x#{{q:window_active}} x#{{q:window_width}} x#{{q:window_height}} x#{{q:window_visible_layout}} x#{{q:window_name}}'; {tmux} -C list-panes -a -F 'Px#{{q:pane_id}} x#{{q:window_id}} x#{{q:pane_active}} x#{{q:alternate_on}} x#{{q:pane_current_command}} x#{{q:pane_title}} x#{{q:pane_current_path}}'"
    );
    let output = crate::ssh_pool::output(config, command, 4 * 1024 * 1024)?;
    let mut draft = Draft::default();
    let mut parser = ControlParser::default();
    let lines = parser
        .push(&output)?
        .into_iter()
        .filter_map(|event| match event {
            ControlEvent::Response {
                success: true,
                lines,
            } => Some(lines),
            _ => None,
        })
        .flatten();
    for line in lines.filter(|l| !l.is_empty()) {
        let fields = shell_words::split(&String::from_utf8_lossy(&line[1..]))?;
        let record = fields
            .iter()
            .map(|f| f.strip_prefix('x').unwrap_or(f))
            .collect::<Vec<_>>()
            .join("\t")
            .into_bytes();
        match line[0] {
            b'S' => draft.sessions.push(record),
            b'W' => draft.windows.push(record),
            b'P' => draft.panes.push(record),
            _ => {}
        }
    }
    Ok(draft)
}
fn discover_async(config: HostConfig, tx: SyncSender<Message>) {
    thread::spawn(move || {
        let result = discover(&config, false);
        let _ = tx.send(Message::Discovery(result));
    });
}

fn auxiliary_request(config: HostConfig, request: Request, tx: SyncSender<Message>) {
    thread::spawn(move || {
        let result = (|| -> Result<Vec<Vec<u8>>> {
            let command = format!("{} -C {}", config.tmux()?, request.command);
            let command = match &request.directory {
                Some(path) => format!("cd -- {} && {command}", shell_directory(path)),
                None => command,
            };
            let output = crate::ssh_pool::output(&config, command, 65536)?;
            let events = ControlParser::default().push(&output)?;
            for event in events {
                if let ControlEvent::Response { success, lines } = event {
                    anyhow::ensure!(
                        success,
                        "{}",
                        lines
                            .iter()
                            .map(|l| String::from_utf8_lossy(l))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    return Ok(lines);
                }
            }
            bail!("{}", crate::t!("tmux.no_confirm"));
        })();
        let (success, lines) = match result {
            Ok(lines) => (true, lines),
            Err(error) => (false, vec![format!("{error:#}").into_bytes()]),
        };
        let _ = tx.send(Message::Reply(request.response, success, lines));
    });
}

fn run(
    config: HostConfig,
    shared: Arc<Mutex<State>>,
    tx: SyncSender<Message>,
    rx: mpsc::Receiver<Message>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let mut reported_themes = BTreeMap::new();
    let _auxiliary = crate::ssh_pool::auxiliary(&config);
    let initial = discover(&config, true)?;
    let mut initial = Some(initial);
    let mut queue = VecDeque::new();
    let mut refreshing = false;
    let mut urgent_refresh = false;
    let mut dirty = true;
    let mut draft = Draft::default();
    let mut hydrating = BTreeSet::new();
    let mut pending_recapture = BTreeSet::new();
    let mut last_active = String::new();
    let mut last_refresh = Instant::now();
    let mut resize: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut last_resize = Instant::now();
    let mut sizes: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut titles = crate::tmux_titles::LiveTitles::default();
    let mut exited = BTreeSet::new();
    let mut discovering = false;
    let mut retry_discovery = None::<Instant>;
    let mut retry_send = None::<Instant>;
    let mut preferred_session = None::<String>;
    let mut discovery_requested = false;
    let mut streams = streams::Streams::new();
    let mut pane_owners = BTreeMap::<String, String>::new();
    loop {
        if stopped.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        {
            let mut state = shared.lock();
            if state.published_revision != state.snapshot.revision {
                state.published_revision = state.snapshot.revision;
                state.updates.notify();
            }
        }
        // Sleep until data arrives or a real deadline is due; no idle 40 ms poll.
        let mut timeout = if dirty && !refreshing && !discovering && streams.first().is_some() {
            if urgent_refresh {
                Duration::ZERO
            } else {
                Duration::from_millis(16).saturating_sub(last_refresh.elapsed())
            }
        } else {
            Duration::from_secs(3600)
        };
        if let Some(retry) = retry_discovery.filter(|_| !refreshing && !discovering) {
            timeout = timeout.min(retry.saturating_duration_since(Instant::now()));
        }
        if let Some(retry) = retry_send {
            timeout = timeout.min(retry.saturating_duration_since(Instant::now()));
        }
        if !resize.is_empty() {
            timeout = timeout.min(Duration::from_millis(120).saturating_sub(last_resize.elapsed()));
        }
        let incoming = if let Some(draft) = initial.take() {
            Ok(Message::Discovery(Ok(draft)))
        } else {
            rx.recv_timeout(timeout.max(Duration::from_millis(1)))
        };
        let message = match incoming {
            Ok(Message::Discovery(result)) => {
                discovering = false;
                retry_discovery = None;
                match result {
                    Ok(next) => {
                        if next.sessions.is_empty() {
                            retry_discovery = Some(Instant::now() + Duration::from_secs(30));
                        }
                        draft = next;
                        Ok(Message::Reply(Response::Current, true, Vec::new()))
                    }
                    Err(error) => {
                        let mut state = shared.lock();
                        state.snapshot.message = Some(
                            crate::t!("tmux.discovery_failed", error = format!("{error:#}"))
                                .to_string(),
                        );
                        retry_discovery = Some(Instant::now() + Duration::from_secs(10));
                        state.snapshot.revision += 1;
                        continue;
                    }
                }
            }
            Ok(Message::StreamEvent(session, ControlEvent::Response { success, lines })) => {
                let Some(response) = streams.response(&session) else {
                    continue;
                };
                Ok(Message::Reply(response, success, lines))
            }
            message => message,
        };
        match message {
            Ok(Message::Wake) => {}
            Ok(Message::StreamOpened(session, result)) => {
                match streams.opened(session.clone(), result) {
                    Ok(true) => {
                        let state = shared.lock();
                        for pane in state
                            .snapshot
                            .sessions
                            .iter()
                            .filter(|item| item.id == session)
                            .flat_map(|item| &item.windows)
                            .flat_map(|window| &window.panes)
                        {
                            pending_recapture.insert(pane.id.clone());
                        }
                        dirty = true;
                        urgent_refresh = true;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        let mut state = shared.lock();
                        state.snapshot.message = Some(
                            crate::t!("tmux.read_failed", error = format!("{error:#}")).to_string(),
                        );
                        state.snapshot.revision += 1;
                        retry_discovery = Some(Instant::now() + Duration::from_secs(10));
                    }
                }
            }
            Ok(Message::Stop) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Message::StreamClosed(session, error)) => {
                hydrating.retain(|pane| pane_owners.get(pane) != Some(&session));
                let pending = streams.remove(
                    &session,
                    error
                        .clone()
                        .unwrap_or_else(|| crate::t!("tmux.channel_closed").to_string()),
                );
                for response in pending {
                    if let Response::Probe(_, sender) = response {
                        let _ = sender.send_blocking(Err(anyhow::anyhow!(
                            error
                                .clone()
                                .unwrap_or_else(|| crate::t!("tmux.channel_closed").to_string())
                        )));
                    }
                }
                if let Some(error) = &error {
                    let mut state = shared.lock();
                    state.snapshot.message =
                        Some(crate::t!("tmux.stream_lost", error = error).to_string());
                    state.snapshot.revision += 1;
                }
                refreshing = false;
                if exited.remove(&session) && error.is_none() {
                    // A normal exit destroys the native session. Reconcile, never recreate it.
                    discovery_requested = true;
                } else if !discovering {
                    retry_discovery = Some(Instant::now() + Duration::from_secs(10));
                }
            }
            Ok(Message::StreamEvent(session, ControlEvent::Output { pane, bytes })) => {
                // A native tmux window can be linked into more than one session.
                // Choose one reader deterministically so bytes and notices are never duplicated.
                if pane_owners.get(&pane) != Some(&session) {
                    continue;
                }
                titles.advance(&pane, &bytes, &mut shared.lock().snapshot);
                if let Some(screen) = shared.lock().screens.get(&pane).cloned() {
                    if hydrating.contains(&pane) {
                        screen.remote_observe(&bytes);
                    } else {
                        screen.remote_output(&bytes);
                    }
                }
            }
            Ok(Message::StreamEvent(session, ControlEvent::Line(line))) => {
                if matches!(
                    line.trim(),
                    "%exit" | "%exit server exited" | "%exit session destroyed"
                ) {
                    exited.insert(session);
                }
                if !titles.subscription(&line, &mut shared.lock().snapshot) {
                    dirty = true;
                }
                if is_pane_exit_notification(&line) {
                    urgent_refresh = true;
                }
            }
            Ok(Message::StreamEvent(_, ControlEvent::Response { .. }))
            | Ok(Message::Discovery(_)) => unreachable!(),
            Ok(Message::Reply(response, success, lines)) => {
                if !success {
                    let error = lines
                        .iter()
                        .map(|l| String::from_utf8_lossy(l))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if let Response::Capture { id, .. } = &response {
                        hydrating.remove(id);
                        pending_recapture.insert(id.clone());
                    }
                    if let Response::Probe(_, sender) = &response {
                        let _ = sender.send_blocking(Err(anyhow::anyhow!(error.clone())));
                    }
                    if matches!(response, Response::Current) {
                        refreshing = false;
                    }
                    let mut state = shared.lock();
                    state.snapshot.message = Some(error.clone());
                    state.snapshot.revision += 1;
                    dirty = true;
                } else {
                    match response {
                        Response::Sessions => draft.sessions = lines,
                        Response::Windows => draft.windows = lines,
                        Response::Panes => draft.panes = lines,
                        Response::Probe(started, sender) => {
                            let _ = sender.send_blocking(Ok(started.elapsed()));
                        }
                        Response::Current => {
                            refreshing = false;
                            let selected = if preferred_session.as_ref().is_some_and(|id| {
                                draft
                                    .sessions
                                    .iter()
                                    .any(|s| s.split(|b| *b == b'\t').next() == Some(id.as_bytes()))
                            }) {
                                preferred_session.take().unwrap()
                            } else {
                                shared.lock().snapshot.active_session.clone()
                            };
                            let sid = if draft.sessions.iter().any(|s| {
                                s.split(|b| *b == b'\t').next() == Some(selected.as_bytes())
                            }) {
                                selected
                            } else {
                                draft
                                    .sessions
                                    .first()
                                    .map(|s| {
                                        String::from_utf8_lossy(s)
                                            .split('\t')
                                            .next()
                                            .unwrap_or("")
                                            .to_owned()
                                    })
                                    .unwrap_or_default()
                            };
                            let mut next = parse_snapshot(&draft, &sid)?;
                            // Use list-panes, not visible layouts: zoomed windows omit hidden panes.
                            pane_owners = draft
                                .panes
                                .iter()
                                .filter_map(|line| {
                                    let line = String::from_utf8_lossy(line);
                                    let mut fields = line.split('\t');
                                    let pane = fields.next()?;
                                    let window = fields.next()?;
                                    let owner = next
                                        .sessions
                                        .iter()
                                        .find(|s| s.windows.iter().any(|w| w.id == window))?;
                                    Some((pane.to_owned(), owner.id.clone()))
                                })
                                .collect();
                            titles.reconcile(&mut next);
                            titles.retain(
                                &draft
                                    .panes
                                    .iter()
                                    .filter_map(|line| {
                                        String::from_utf8_lossy(line)
                                            .split('\t')
                                            .next()
                                            .map(str::to_owned)
                                    })
                                    .collect(),
                            );
                            let previous = shared.lock().snapshot.clone();
                            let rebalances = rebalance_removed_panes(&previous, &next);
                            if !rebalances.is_empty() {
                                normalize_removed_panes(&previous, &mut next);
                            }
                            let active = next
                                .window()
                                .map(|w| format!("{}:{}", sid, w.id))
                                .unwrap_or_default();
                            let changed = active != last_active;
                            last_active = active;
                            let active_window = next.window().cloned();
                            if let Some(deadline) = streams.reconcile(&config, &next.sessions, &tx)
                            {
                                retry_discovery = Some(
                                    retry_discovery
                                        .map_or(deadline, |scheduled| scheduled.min(deadline)),
                                );
                            }
                            let mut state = shared.lock();
                            state.screens.retain(|id, _| pane_owners.contains_key(id));
                            pending_recapture.retain(|id| pane_owners.contains_key(id));
                            {
                                for pane in next
                                    .sessions
                                    .iter()
                                    .flat_map(|s| &s.windows)
                                    .flat_map(|w| &w.panes)
                                {
                                    let fresh = !state.screens.contains_key(&pane.id);
                                    if fresh {
                                        let input_tx = tx.clone();
                                        let id = pane.id.clone();
                                        state.screens.insert(
                                            pane.id.clone(),
                                            Session::remote(
                                                pane.id.clone(),
                                                pane.rows,
                                                pane.cols,
                                                Arc::new(move |data| {
                                                    anyhow::ensure!(
                                                        data.len() <= 64 * 1024,
                                                        "{}",
                                                        crate::t!("tmux.paste_too_large")
                                                    );
                                                    input_tx
                                                        .try_send(Message::Input(id.clone(), data))
                                                        .context(crate::t!("tmux.queue_full"))
                                                }),
                                            ),
                                        );
                                    }
                                    let screen = state.screens.get(&pane.id).unwrap();
                                    *screen.title.lock() = pane.title.clone();
                                    let resized =
                                        !capture_matches_size(screen, pane.cols, pane.rows);
                                    if resized {
                                        screen.remote_resize(pane.rows, pane.cols);
                                    }
                                    if active_window
                                        .as_ref()
                                        .is_some_and(|w| w.panes.iter().any(|p| p.id == pane.id))
                                        && should_capture_pane(
                                            fresh,
                                            changed,
                                            pending_recapture.contains(&pane.id),
                                        )
                                        && hydrating.insert(pane.id.clone())
                                    {
                                        let alternate = draft.panes.iter().any(|line| {
                                            let fields = String::from_utf8_lossy(line);
                                            let f: Vec<_> = fields.splitn(5, '\t').collect();
                                            f.len() == 5 && f[0] == pane.id && f[3] == "1"
                                        });
                                        let owner = next
                                            .sessions
                                            .iter()
                                            .find(|s| {
                                                s.windows.iter().any(|w| {
                                                    w.panes.iter().any(|p| p.id == pane.id)
                                                })
                                            })
                                            .unwrap();
                                        // Capture and subsequent live bytes must share one ordered
                                        // control stream, including while the command client switches.
                                        if let Err((error, _)) = streams.send(
                                            &owner.id,
                                            capture_request(
                                                &pane.id, alternate, pane.cols, pane.rows,
                                            ),
                                        ) {
                                            hydrating.remove(&pane.id);
                                            pending_recapture.insert(pane.id.clone());
                                            state.snapshot.message = Some(error.to_string());
                                            dirty = true;
                                        } else {
                                            pending_recapture.remove(&pane.id);
                                        }
                                    }
                                }
                            }
                            if state.snapshot.sessions != next.sessions
                                || state.snapshot.active_session != next.active_session
                                || state.snapshot.connection != next.connection
                                || state.snapshot.message != next.message
                            {
                                for request in rebalances.into_iter().rev() {
                                    queue.push_front(request);
                                }
                                let revision = state.snapshot.revision + 1;
                                state.snapshot = Snapshot { revision, ..next };
                            }
                        }
                        Response::Capture {
                            id,
                            alternate,
                            cols,
                            rows,
                        } => {
                            let screen = shared.lock().screens.get(&id).cloned();
                            if let Some(screen) = screen {
                                if capture_matches_size(&screen, cols, rows) {
                                    screen.remote_restore_capture(&lines, alternate);
                                    let state = shared.lock();
                                    if let Some(owner) = state.snapshot.sessions.iter().find(|s| {
                                        s.windows.iter().any(|w| w.panes.iter().any(|p| p.id == id))
                                    }) {
                                        let mode_request = request(
                                            format!("display-message -p -t {} '#{{cursor_x}}\t#{{cursor_y}}\t#{{cursor_flag}}\t#{{keypad_cursor_flag}}\t#{{bracketed_paste_flag}}\t#{{mouse_standard_flag}}\t#{{mouse_button_flag}}\t#{{mouse_all_flag}}\t#{{mouse_utf8_flag}}\t#{{mouse_sgr_flag}}\t#{{pane_private_modes}}'", q(&id)),
                                            Response::Modes(id.clone()),
                                        )
                                        .session(Some(&owner.id));
                                        match streams.send(&owner.id, mode_request) {
                                            Ok(()) => {}
                                            Err((error, request))
                                                if error
                                                    .downcast_ref::<io::Error>()
                                                    .is_some_and(|error| {
                                                        error.kind() == io::ErrorKind::WouldBlock
                                                    }) =>
                                            {
                                                queue.push_front(request);
                                                retry_send = Some(
                                                    Instant::now() + Duration::from_millis(50),
                                                );
                                            }
                                            Err((error, _)) => {
                                                tracing::warn!(%error, "Could not restore pane modes");
                                            }
                                        }
                                    }
                                } else {
                                    pending_recapture.insert(id.clone());
                                    dirty = true;
                                    urgent_refresh = true;
                                }
                            }
                            hydrating.remove(&id);
                        }
                        Response::Modes(id) => {
                            if let (Some(screen), Some(line)) =
                                (shared.lock().screens.get(&id).cloned(), lines.first())
                            {
                                if let Some(modes) = restored_terminal_modes(line) {
                                    screen.remote_restore(&modes);
                                }
                            }
                        }
                        Response::CreatedSession => {
                            discovery_requested = true;
                            if let Some(line) = lines.first() {
                                preferred_session =
                                    Some(String::from_utf8_lossy(line).into_owned());
                            }
                            dirty = true;
                        }
                        Response::Ignore => {}
                        Response::Relayout => {
                            dirty = true;
                            urgent_refresh = true;
                        }
                    }
                }
            }
            Ok(Message::Action(action)) => {
                if let Action::SelectSession(id) | Action::NewWindowInSession(id) = &action {
                    let mut state = shared.lock();
                    if state.snapshot.sessions.iter().any(|s| &s.id == id) {
                        state.snapshot.active_session = id.clone();
                        state.snapshot.revision += 1;
                    }
                }
                let state = shared.lock().snapshot.clone();
                let action_target = match &action {
                    Action::RemoveSession(id) | Action::RenameSession { id, .. } => id.clone(),
                    _ => state.active_session.clone(),
                };
                match action_requests(action, &state) {
                    Ok(requests) => {
                        for mut request in requests {
                            if matches!(request.response, Response::CreatedSession) {
                                auxiliary_request(config.clone(), request, tx.clone());
                            } else {
                                request.target = Some(action_target.clone());
                                queue.push_back(request);
                            }
                        }
                    }
                    Err(error) => {
                        let mut state = shared.lock();
                        state.snapshot.message = Some(error.to_string());
                        state.snapshot.revision += 1;
                    }
                }
                dirty = true;
            }
            Ok(Message::Probe(sender)) => {
                let selected = shared.lock().snapshot.active_session.clone();
                let target = if streams.contains(&selected) {
                    Some(selected)
                } else {
                    streams.first()
                };
                if let Some(target) = target {
                    queue.push_back(
                        request(
                            "display-message -p '#{version}'",
                            Response::Probe(Instant::now(), sender),
                        )
                        .session(Some(&target)),
                    );
                } else {
                    let _ = sender.send_blocking(Err(anyhow::anyhow!(
                        crate::t!("tmux.channel_closed").to_string()
                    )));
                }
                dirty = true;
            }
            Ok(Message::Input(id, bytes)) => {
                if queue.len() > 4096 {
                    let mut s = shared.lock();
                    s.snapshot.message = Some(crate::t!("tmux.input_busy").to_string().into());
                    s.snapshot.revision += 1;
                    continue;
                }
                for chunk in bytes.chunks(128) {
                    let hex = chunk
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    queue.push_back(
                        request(
                            format!("send-keys -H -t {} {hex}", q(&id)),
                            Response::Ignore,
                        )
                        .session(pane_owners.get(&id)),
                    );
                }
            }
            Ok(Message::Resize(id, cols, rows)) => {
                resize.insert(id, (cols, rows));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        {
            let (theme, panes) = {
                let state = shared.lock();
                (
                    state.theme,
                    state
                        .snapshot
                        .sessions
                        .iter()
                        .flat_map(|s| &s.windows)
                        .flat_map(|w| &w.panes)
                        .map(|p| p.id.clone())
                        .collect::<BTreeSet<_>>(),
                )
            };
            reported_themes.retain(|id, _| panes.contains(id));
            for id in panes {
                if reported_themes.get(&id) != Some(&theme) {
                    for (code, color) in [(10, theme.foreground), (11, theme.background)] {
                        let report = format!(
                            "{id}:\x1b]{code};rgb:{:04x}/{:04x}/{:04x}\x1b\\",
                            ((color >> 16) & 255) * 257,
                            ((color >> 8) & 255) * 257,
                            (color & 255) * 257
                        );
                        queue.push_back(
                            request(
                                format!("refresh-client -r {}", q(&report)),
                                Response::Ignore,
                            )
                            .session(pane_owners.get(&id)),
                        );
                    }
                    if let Some(screen) = shared.lock().screens.get(&id).cloned() {
                        screen.set_theme(theme);
                    }
                    reported_themes.insert(id, theme);
                }
            }
            if last_resize.elapsed() > Duration::from_millis(120) {
                for (id, (cols, rows)) in std::mem::take(&mut resize) {
                    if sizes.get(&id) != Some(&(cols, rows)) {
                        let owner = shared
                            .lock()
                            .snapshot
                            .sessions
                            .iter()
                            .find(|s| s.windows.iter().any(|w| w.id == id))
                            .map(|s| s.id.clone());
                        queue.push_back(
                            request(
                                format!("refresh-client -C {}", q(&format!("{id}:{cols}x{rows}"))),
                                Response::Ignore,
                            )
                            .session(owner.as_ref()),
                        );
                        sizes.insert(id, (cols, rows));
                        dirty = true;
                    }
                }
                last_resize = Instant::now();
            }
            if !refreshing
                && !discovering
                && (discovery_requested
                    || retry_discovery.is_some_and(|deadline| Instant::now() >= deadline))
            {
                discovering = true;
                discovery_requested = false;
                retry_discovery = None;
                discover_async(config.clone(), tx.clone());
            }
            if !refreshing
                && !discovering
                && streams.first().is_some()
                && dirty
                && (urgent_refresh || last_refresh.elapsed() > Duration::from_millis(16))
            {
                titles.begin_query();
                let start = queue.len();
                refresh(&mut queue);
                let selected = shared.lock().snapshot.active_session.clone();
                let target = if streams.contains(&selected) {
                    Some(selected)
                } else {
                    streams.first()
                };
                for r in queue.iter_mut().skip(start) {
                    r.target = target.clone();
                }
                refreshing = true;
                dirty = false;
                urgent_refresh = false;
                last_refresh = Instant::now();
            }
            for _ in 0..queue.len() {
                let Some(request) = queue.pop_front() else {
                    break;
                };
                let selected = shared.lock().snapshot.active_session.clone();
                let target = request.target.clone().unwrap_or(selected);
                let target = if streams.contains(&target) {
                    Some(target)
                } else if request.target.is_some() {
                    let session_exists = shared
                        .lock()
                        .snapshot
                        .sessions
                        .iter()
                        .any(|session| session.id == target);
                    if streams.is_opening(&target)
                        || (session_exists && streams.is_retrying(&target))
                    {
                        queue.push_back(request);
                        retry_send = streams.retry_deadline();
                        continue;
                    }
                    let mut state = shared.lock();
                    state.snapshot.message =
                        Some(crate::t!("tmux.session_not_ready").to_string().into());
                    state.snapshot.revision += 1;
                    continue;
                } else {
                    streams.first()
                };
                let Some(target) = target else {
                    queue.push_front(request);
                    retry_send = None;
                    break;
                };
                if !streams.can_send(&target) {
                    queue.push_back(request);
                    retry_send = None;
                    continue;
                }
                match streams.send(&target, request) {
                    Ok(()) => retry_send = None,
                    Err((error, request))
                        if error
                            .downcast_ref::<io::Error>()
                            .is_some_and(|error| error.kind() == io::ErrorKind::WouldBlock) =>
                    {
                        queue.push_front(request);
                        retry_send = Some(Instant::now() + Duration::from_millis(50));
                        break;
                    }
                    Err((error, _)) => {
                        retry_send = None;
                        let mut state = shared.lock();
                        state.snapshot.message = Some(
                            crate::t!("tmux.command_failed", error = format!("{error:#}"))
                                .to_string(),
                        );
                        state.snapshot.revision += 1;
                    }
                }
            }
            if queue.is_empty() {
                retry_send = None;
            }
        }
    }
    Ok(())
}

fn parse_snapshot(draft: &Draft, active_session: &str) -> Result<Snapshot> {
    let mut sessions = Vec::new();
    for line in &draft.sessions {
        let line = String::from_utf8_lossy(line);
        let mut fields = line.splitn(3, '\t');
        let (Some(id), Some(name), Some(cwd)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        anyhow::ensure!(
            id.starts_with('$') && id[1..].parse::<u64>().is_ok(),
            "invalid tmux session id"
        );
        sessions.push(SessionInfo {
            id: id.into(),
            name: name.into(),
            cwd: cwd.into(),
            ..Default::default()
        });
    }
    for line in &draft.windows {
        let line = String::from_utf8_lossy(line);
        let f: Vec<_> = line.splitn(8, '\t').collect();
        if f.len() != 8 {
            continue;
        }
        let Some(session) = sessions.iter_mut().find(|s| s.id == f[0]) else {
            continue;
        };
        anyhow::ensure!(
            f[1].starts_with('@') && f[1][1..].parse::<u64>().is_ok(),
            "invalid tmux window id"
        );
        let mut window = WindowInfo {
            id: f[1].into(),
            name: f[7].into(),
            index: f[2].parse()?,
            cols: f[4].parse()?,
            rows: f[5].parse()?,
            panes: parse_layout(f[6])?,
            ..Default::default()
        };
        let total = draft
            .panes
            .iter()
            .filter(|p| String::from_utf8_lossy(p).split('\t').nth(1) == Some(f[1]))
            .count();
        window.zoomed = window.panes.len() < total;
        for p in &draft.panes {
            let p = String::from_utf8_lossy(p);
            let fields: Vec<_> = p.splitn(7, '\t').collect();
            if fields.len() < 6 || fields[1] != window.id {
                continue;
            }
            if let Some(pane) = window.panes.iter_mut().find(|pane| pane.id == fields[0]) {
                pane.cwd = fields.get(6).copied().unwrap_or_default().into();
                pane.title = if fields[5].is_empty() {
                    fields[4].into()
                } else {
                    fields[5].into()
                };
                if fields[2] == "1" {
                    window.active_pane = pane.id.clone();
                }
            }
        }
        if f[3] == "1" {
            session.active_window = window.id.clone();
        }
        session.windows.push(window);
    }
    for s in &mut sessions {
        s.windows.sort_by_key(|w| w.index);
        if s.active_window.is_empty() {
            s.active_window = s.windows.first().map(|w| w.id.clone()).unwrap_or_default();
        }
    }
    Ok(Snapshot {
        sessions,
        active_session: active_session.into(),
        connection: Connection::Ready,
        message: None,
        revision: 0,
    })
}

fn equal_sizes(extent: usize, count: usize) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    let available = extent.saturating_sub(count.saturating_sub(1));
    let base = available / count;
    let remainder = available % count;
    (0..count)
        .map(|index| (base + usize::from(index < remainder)).max(1))
        .collect()
}

fn pane_columns(window: &WindowInfo) -> BTreeMap<usize, Vec<&crate::backend::PaneInfo>> {
    let mut columns: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for pane in &window.panes {
        columns.entry(pane.x).or_default().push(pane);
    }
    for rows in columns.values_mut() {
        rows.sort_by_key(|pane| pane.y);
    }
    columns
}

fn sizes_are_equal(values: impl Iterator<Item = usize>) -> bool {
    let mut values = values;
    let Some(first) = values.next() else {
        return true;
    };
    let (mut min, mut max) = (first, first);
    for value in values {
        min = min.min(value);
        max = max.max(value);
    }
    max - min <= 1
}

fn is_equal_column_layout(window: &WindowInfo) -> bool {
    let columns = pane_columns(window);
    if columns.is_empty()
        || !sizes_are_equal(
            columns
                .values()
                .filter_map(|rows| rows.first().map(|pane| pane.cols)),
        )
    {
        return false;
    }
    let mut expected_x = 0;
    for (&x, rows) in &columns {
        let Some(first) = rows.first() else {
            return false;
        };
        if x != expected_x
            || first.y != 0
            || rows
                .last()
                .is_none_or(|last| last.y + last.rows != window.rows)
            || rows.iter().any(|pane| pane.cols != first.cols)
            || !sizes_are_equal(rows.iter().map(|pane| pane.rows))
        {
            return false;
        }
        let mut expected_y = 0;
        for pane in rows {
            if pane.y != expected_y {
                return false;
            }
            expected_y = pane.y + pane.rows + 1;
        }
        expected_x = x + first.cols + 1;
    }
    expected_x.saturating_sub(1) == window.cols
}

fn is_pure_horizontal_layout(window: &WindowInfo) -> bool {
    window
        .panes
        .iter()
        .all(|pane| pane.y == 0 && pane.rows == window.rows)
}

fn is_pure_vertical_layout(window: &WindowInfo) -> bool {
    window
        .panes
        .iter()
        .all(|pane| pane.x == 0 && pane.cols == window.cols)
}

fn rebalance_removed_panes(previous: &Snapshot, next: &Snapshot) -> Vec<Request> {
    let mut requests = Vec::new();
    for next_session in &next.sessions {
        let Some(previous_session) = previous
            .sessions
            .iter()
            .find(|session| session.id == next_session.id)
        else {
            continue;
        };
        for next_window in &next_session.windows {
            let Some(previous_window) = previous_session
                .windows
                .iter()
                .find(|window| window.id == next_window.id)
            else {
                continue;
            };
            if previous_window.zoomed
                || next_window.zoomed
                || previous_window.panes.len() <= next_window.panes.len()
                || !is_equal_column_layout(previous_window)
            {
                continue;
            }
            if is_pure_horizontal_layout(previous_window) {
                requests.push(request(
                    format!("select-layout -t {} even-horizontal", q(&next_window.id)),
                    Response::Relayout,
                ));
                continue;
            }
            if is_pure_vertical_layout(previous_window) {
                requests.push(request(
                    format!("select-layout -t {} even-vertical", q(&next_window.id)),
                    Response::Relayout,
                ));
                continue;
            }
            let request_start = requests.len();
            let columns = pane_columns(next_window);
            if columns.len() > 1 {
                let widths = equal_sizes(next_window.cols, columns.len());
                requests.extend(
                    columns
                        .values()
                        .filter_map(|rows| rows.first())
                        .zip(widths)
                        .map(|(pane, width)| {
                            request(
                                format!("resize-pane -x {width} -t {}", q(&pane.id)),
                                Response::Ignore,
                            )
                        }),
                );
            }
            for rows in columns.values().filter(|rows| rows.len() > 1) {
                let heights = equal_sizes(next_window.rows, rows.len());
                requests.extend(rows.iter().zip(heights).map(|(pane, height)| {
                    request(
                        format!("resize-pane -y {height} -t {}", q(&pane.id)),
                        Response::Ignore,
                    )
                }));
            }
            if requests.len() > request_start {
                requests.last_mut().unwrap().response = Response::Relayout;
            }
        }
    }
    requests
}

fn normalize_removed_panes(previous: &Snapshot, next: &mut Snapshot) {
    for next_session in &mut next.sessions {
        let Some(previous_session) = previous
            .sessions
            .iter()
            .find(|session| session.id == next_session.id)
        else {
            continue;
        };
        for next_window in &mut next_session.windows {
            let Some(previous_window) = previous_session
                .windows
                .iter()
                .find(|window| window.id == next_window.id)
            else {
                continue;
            };
            if previous_window.zoomed
                || next_window.zoomed
                || previous_window.panes.len() <= next_window.panes.len()
                || !is_equal_column_layout(previous_window)
            {
                continue;
            }

            let mut columns: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
            for (index, pane) in next_window.panes.iter().enumerate() {
                columns.entry(pane.x).or_default().push(index);
            }
            for rows in columns.values_mut() {
                rows.sort_by_key(|index| next_window.panes[*index].y);
            }

            let widths = equal_sizes(next_window.cols, columns.len());
            let mut x = 0;
            for (rows, width) in columns.into_values().zip(widths) {
                let heights = equal_sizes(next_window.rows, rows.len());
                let mut y = 0;
                for (index, height) in rows.into_iter().zip(heights) {
                    let pane = &mut next_window.panes[index];
                    pane.x = x;
                    pane.y = y;
                    pane.cols = width;
                    pane.rows = height;
                    y += height + 1;
                }
                x += width + 1;
            }
        }
    }
}

fn action_requests(action: Action, state: &Snapshot) -> Result<Vec<Request>> {
    let window = state.window();
    let pane = window.map(|w| q(&w.active_pane)).unwrap_or_default();
    let command = match action {
        Action::RenameSession { id, name } => {
            anyhow::ensure!(
                !name.trim().is_empty()
                    && !name.chars().any(char::is_control)
                    && !name.contains([':', '.']),
                "{}",
                crate::t!("tmux.session_name_invalid")
            );
            format!("rename-session -t {} {}", q(&id), q(&name))
        }
        Action::RenameWindow { id, name } => {
            anyhow::ensure!(
                !name.trim().is_empty() && !name.chars().any(char::is_control),
                "{}",
                crate::t!("tmux.label_invalid")
            );
            format!("rename-window -t {} {}", q(&id), q(&name))
        }
        Action::RemoveSession(id) => format!("kill-session -t {}", q(&id)),
        Action::NewWindowInSession(id) => format!("new-window -t {}", q(&format!("{id}:"))),
        Action::MoveFocus(direction) => format!("select-pane {} -t {pane}", direction.flag()),
        Action::MovePane(direction) => {
            let Some(target) = window.and_then(|w| crate::backend::adjacent_pane(w, direction))
            else {
                return Ok(vec![]);
            };
            format!("swap-pane -s {pane} -t {}", q(&target))
        }
        Action::CycleLayout => format!(
            "next-layout -t {}",
            q(&window.context(crate::t!("tmux.no_window"))?.id)
        ),
        Action::SelectSession(_) => return Ok(vec![]),
        Action::SelectWindow(id) => format!("select-window -t {}", q(&id)),
        Action::SelectPane(id) => format!("select-pane -t {}", q(&id)),
        Action::NewSession | Action::NewSessionAt(_) | Action::NewNamedSession { .. } => {
            let directory = match &action {
                Action::NewSessionAt(path) | Action::NewNamedSession { path, .. } => {
                    Some(path.clone())
                }
                _ => None,
            };
            let command = match action {
                Action::NewNamedSession { path, name } => {
                    anyhow::ensure!(
                        !name.trim().is_empty()
                            && !name.chars().any(char::is_control)
                            && !name.contains([':', '.']),
                        "{}",
                        crate::t!("tmux.workspace_name_invalid")
                    );
                    format!(
                        "new-session -d -s {} -c {} -P -F '#{{session_id}}'",
                        shell_q(&name),
                        if path.starts_with('/') {
                            shell_q(&path)
                        } else {
                            "\"$PWD\"".into()
                        }
                    )
                }
                Action::NewSessionAt(path) => {
                    format!(
                        "new-session -d -c {} -P -F '#{{session_id}}'",
                        if path.starts_with('/') {
                            shell_q(&path)
                        } else {
                            "\"$PWD\"".into()
                        }
                    )
                }
                _ => "new-session -d -P -F '#{session_id}'".into(),
            };
            let mut request = request(command, Response::CreatedSession);
            request.directory = directory;
            return Ok(vec![request]);
        }
        Action::NewWindowAt { path, name } => {
            anyhow::ensure!(
                !path.is_empty() && !path.chars().any(char::is_control),
                "{}",
                crate::t!("tmux.directory_invalid")
            );
            anyhow::ensure!(
                !name.chars().any(char::is_control),
                "{}",
                crate::t!("tmux.pane_name_invalid")
            );
            format!(
                "new-window -t {} -c {}{}",
                q(&format!("{}:", state.active_session)),
                q(&path),
                if name.is_empty() {
                    String::new()
                } else {
                    format!(" -n {}", q(&name))
                }
            )
        }
        Action::NewWindow => format!("new-window -t {}", q(&format!("{}:", state.active_session))),
        Action::Split(SplitAxis::Horizontal) => {
            let window = window.context(crate::t!("tmux.no_window"))?;
            if is_pure_horizontal_layout(window) {
                return Ok(vec![request_sequence(
                    vec![
                        format!("split-window -h -f -t {pane} -c '#{{pane_current_path}}'"),
                        format!("select-layout -t {} even-horizontal", q(&window.id)),
                    ],
                    Response::Relayout,
                )]);
            }
            let mut columns = BTreeMap::new();
            for pane in &window.panes {
                columns.entry(pane.x).or_insert(pane);
            }
            let column_count = columns.len().max(1);
            let widths = equal_sizes(window.cols, column_count + 1);
            let new_width = widths[column_count];
            let mut requests = vec![request(
                format!("split-window -h -f -l {new_width} -t {pane} -c '#{{pane_current_path}}'"),
                Response::Ignore,
            )];
            requests.extend(columns.into_values().zip(widths).map(|(column, width)| {
                request(
                    format!("resize-pane -x {width} -t {}", q(&column.id)),
                    Response::Ignore,
                )
            }));
            return Ok(requests);
        }
        Action::Split(SplitAxis::Vertical) => {
            let window = window.context(crate::t!("tmux.no_window"))?;
            if is_pure_vertical_layout(window) {
                return Ok(vec![request_sequence(
                    vec![
                        format!("split-window -v -t {pane} -c '#{{pane_current_path}}'"),
                        format!("select-layout -t {} even-vertical", q(&window.id)),
                    ],
                    Response::Relayout,
                )]);
            }
            let active = window
                .panes
                .iter()
                .find(|candidate| candidate.id == window.active_pane)
                .context(crate::t!("tmux.no_pane"))?;
            let mut rows: Vec<_> = window
                .panes
                .iter()
                .filter(|candidate| candidate.x == active.x && candidate.cols == active.cols)
                .collect();
            rows.sort_by_key(|candidate| candidate.y);
            let top = rows.iter().map(|row| row.y).min().unwrap_or(active.y);
            let bottom = rows
                .iter()
                .map(|row| row.y + row.rows)
                .max()
                .unwrap_or(active.y + active.rows);
            let row_count = rows.len() + 1;
            let heights = equal_sizes(bottom.saturating_sub(top), row_count);
            let active_index = rows
                .iter()
                .position(|row| row.id == active.id)
                .context(crate::t!("tmux.pane_not_in_layout"))?;
            let new_index = active_index + 1;
            let mut requests = vec![request(
                format!(
                    "split-window -v -l {} -t {pane} -c '#{{pane_current_path}}'",
                    heights[new_index]
                ),
                Response::Ignore,
            )];
            requests.extend(rows.into_iter().enumerate().map(|(index, row)| {
                let target_index = if index <= active_index {
                    index
                } else {
                    index + 1
                };
                let height = heights[target_index];
                request(
                    format!("resize-pane -y {height} -t {}", q(&row.id)),
                    Response::Ignore,
                )
            }));
            return Ok(requests);
        }
        Action::ClosePane => {
            let window = window.context(crate::t!("tmux.no_window"))?;
            let Some(active) = window
                .panes
                .iter()
                .find(|candidate| candidate.id == window.active_pane)
            else {
                return Ok(vec![]);
            };
            if window.panes.len() > 1 && is_pure_horizontal_layout(window) {
                return Ok(vec![request_sequence(
                    vec![
                        format!("kill-pane -t {pane}"),
                        format!("select-layout -t {} even-horizontal", q(&window.id)),
                    ],
                    Response::Relayout,
                )]);
            }
            if window.panes.len() > 1 && is_pure_vertical_layout(window) {
                return Ok(vec![request_sequence(
                    vec![
                        format!("kill-pane -t {pane}"),
                        format!("select-layout -t {} even-vertical", q(&window.id)),
                    ],
                    Response::Relayout,
                )]);
            }
            let mut requests = vec![request(format!("kill-pane -t {pane}"), Response::Ignore)];
            let mut rows: Vec<_> = window
                .panes
                .iter()
                .filter(|candidate| {
                    candidate.id != active.id
                        && candidate.x == active.x
                        && candidate.cols == active.cols
                })
                .collect();
            if !rows.is_empty() {
                rows.sort_by_key(|candidate| candidate.y);
                let top = window
                    .panes
                    .iter()
                    .filter(|candidate| candidate.x == active.x && candidate.cols == active.cols)
                    .map(|row| row.y)
                    .min()
                    .unwrap_or(active.y);
                let bottom = window
                    .panes
                    .iter()
                    .filter(|candidate| candidate.x == active.x && candidate.cols == active.cols)
                    .map(|row| row.y + row.rows)
                    .max()
                    .unwrap_or(active.y + active.rows);
                let heights = equal_sizes(bottom.saturating_sub(top), rows.len());
                requests.extend(rows.into_iter().zip(heights).map(|(row, height)| {
                    request(
                        format!("resize-pane -y {height} -t {}", q(&row.id)),
                        Response::Ignore,
                    )
                }));
            } else {
                let mut columns = BTreeMap::new();
                for candidate in &window.panes {
                    if candidate.x != active.x {
                        columns.entry(candidate.x).or_insert(candidate);
                    }
                }
                if !columns.is_empty() {
                    let widths = equal_sizes(window.cols, columns.len());
                    requests.extend(columns.into_values().zip(widths).map(|(column, width)| {
                        request(
                            format!("resize-pane -x {width} -t {}", q(&column.id)),
                            Response::Ignore,
                        )
                    }));
                }
            }
            let commands = requests
                .into_iter()
                .map(|request| request.command)
                .collect::<Vec<_>>();
            return Ok(vec![request_sequence(commands, Response::Relayout)]);
        }
        Action::CloseWindow => format!(
            "kill-window -t {}",
            q(&window.context(crate::t!("tmux.no_tmux_window"))?.id)
        ),
        Action::Zoom => format!("resize-pane -Z -t {pane}"),
        Action::ResizePane { id, axis, amount } => format!(
            "resize-pane -t {} {} {}",
            q(&id),
            match (axis, amount >= 0) {
                (SplitAxis::Horizontal, true) => "-R",
                (SplitAxis::Horizontal, false) => "-L",
                (SplitAxis::Vertical, true) => "-D",
                (SplitAxis::Vertical, false) => "-U",
            },
            amount.unsigned_abs().min(100)
        ),
    };
    Ok(vec![request(command, Response::Ignore)])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_pane_resize_keeps_streaming_without_capture() {
        use alacritty_terminal::{
            index::{Column, Line},
            vte::ansi::{Color, NamedColor},
        };

        assert!(should_capture_pane(true, false, false));
        assert!(should_capture_pane(false, true, false));
        assert!(should_capture_pane(false, false, true));

        let screen = Session::remote("%7".into(), 3, 8, Arc::new(|_| Ok(())));
        screen.remote_output(b"\x1b[?1049h\x1b[3;1H\x1b[46mFOOTER\x1b[0m");
        screen.remote_resize(4, 10);
        assert!(!should_capture_pane(false, false, false));
        screen.remote_output(b"\x1b[1;1HX");

        let term = screen.term.lock();
        assert_eq!(term.grid()[Line(0)][Column(0)].c, 'X');
        assert_eq!(
            term.grid()[Line(0)][Column(0)].bg,
            Color::Named(NamedColor::Background)
        );
    }

    #[test]
    fn capture_preserves_trailing_cells_and_rejects_stale_resize_results() {
        use alacritty_terminal::{
            index::{Column, Line},
            vte::ansi::{Color, NamedColor},
        };

        let capture = capture_request("%7", true, 80, 24);
        assert_eq!(capture.command, "capture-pane -p -e -N -t '%7' -S -2000");
        assert!(matches!(
            capture.response,
            Response::Capture {
                id,
                alternate: true,
                cols: 80,
                rows: 24,
            } if id == "%7"
        ));

        let screen = Session::remote("%7".into(), 24, 80, Arc::new(|_| Ok(())));
        assert!(capture_matches_size(&screen, 80, 24));
        screen.remote_resize(24, 100);
        assert!(!capture_matches_size(&screen, 80, 24));
        assert!(capture_matches_size(&screen, 100, 24));

        let mut parser = ControlParser::default();
        assert!(
            parser
                .push(b"%begin 1 1 0\n\x1b[46mA  ")
                .unwrap()
                .is_empty()
        );
        let events = parser.push(b" \n\n\x1b[49mB\n%end 1 1 0\n").unwrap();
        let [
            ControlEvent::Response {
                success: true,
                lines,
            },
        ] = events.as_slice()
        else {
            panic!("missing capture response: {events:?}");
        };
        assert_eq!(lines[0], b"\x1b[46mA   ");
        assert!(lines[1].is_empty());

        let restored = Session::remote("%capture".into(), 3, 8, Arc::new(|_| Ok(())));
        restored.remote_restore_capture(lines, true);
        let term = restored.term.lock();
        assert_eq!(
            term.grid()[Line(0)][Column(3)].bg,
            Color::Named(NamedColor::Cyan)
        );
        assert_eq!(
            term.grid()[Line(1)][Column(0)].bg,
            Color::Named(NamedColor::Background)
        );
    }

    #[test]
    #[ignore = "requires TSHELL_SSH_TEST_HOST; uses an isolated tmux socket"]
    fn real_tmux_last_shell_exit_removes_session() {
        use super::*;
        struct Cleanup(HostConfig);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self
                    .0
                    .ssh_command()
                    .unwrap()
                    .arg(format!("{} kill-server", self.0.tmux().unwrap()))
                    .output();
            }
        }
        let config = HostConfig {
            destination: std::env::var("TSHELL_SSH_TEST_HOST").unwrap(),
            name: String::new(),
            user: String::new(),
            port: None,
            identity_file: None,
            tmux: true,

            socket: Some(format!(
                "tshell-recovery-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            )),
        };
        let _cleanup = Cleanup(config.clone());
        let wait_ready = |client: &TmuxClient| {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let state = client.snapshot();
                if state.connected()
                    && let Some(window) = state.window()
                    && client.screen(&window.active_pane).is_some()
                {
                    return state;
                }
                assert!(Instant::now() < deadline, "connection not ready: {state:?}");
                thread::sleep(Duration::from_millis(20));
            }
        };
        let client = TmuxClient::connect(config.clone());
        let initial = wait_ready(&client);
        client
            .apply(Action::RenameSession {
                id: initial.active_session,
                name: "recovery workspace".into(),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while client.snapshot().session().unwrap().name != "recovery workspace" {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        let original = client.snapshot().session().unwrap().clone();
        let pane = client.snapshot().window().unwrap().active_pane.clone();
        client
            .screen(&pane)
            .unwrap()
            .input(b"exit\r".to_vec())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !client.snapshot().sessions.is_empty() {
            assert!(
                Instant::now() < deadline,
                "session survived exit: {:?}",
                client.snapshot()
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert!(client.snapshot().connected());
        assert!(client.screen(&pane).is_none());
        client
            .apply(Action::NewNamedSession {
                name: "after exit".into(),
                path: original.cwd,
            })
            .unwrap();
        let created = wait_ready(&client);
        assert_eq!(created.session().unwrap().name, "after exit");
        client.apply(Action::ClosePane).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !client.snapshot().sessions.is_empty() {
            assert!(
                Instant::now() < deadline,
                "last pane close recreated the session"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
    #[test]
    fn host_connects_without_a_session_binding() {
        let old: super::HostConfig =
            serde_json::from_str(r#"{"destination":"server","port":null,"tmux_name":"legacy"}"#)
                .unwrap();
        assert_eq!(old.destination, "server");
        assert!(!serde_json::to_string(&old).unwrap().contains("tmux_name"));
    }

    #[test]
    fn ssh_config_query_uses_selected_identity_file() {
        let host = HostConfig {
            destination: "server".into(),
            name: String::new(),
            user: String::new(),
            port: None,
            identity_file: Some("key with spaces".into()),
            tmux: false,
            socket: None,
        };
        let command = host.ssh_command().unwrap();
        let args: Vec<_> = command.get_args().collect();
        assert!(
            args.windows(2)
                .any(|args| args == ["-i", "key with spaces"])
        );
    }

    #[test]
    fn command_sequences_track_each_response_and_exit_notifications_are_urgent() {
        let sequence = request_sequence(
            vec![
                "kill-pane -t %1".into(),
                "select-layout even-horizontal".into(),
            ],
            Response::Relayout,
        );
        assert_eq!(sequence.leading_ignores, 1);
        assert_eq!(
            sequence.command,
            "kill-pane -t %1 ; select-layout even-horizontal"
        );
        assert!(matches!(sequence.response, Response::Relayout));
        assert!(is_pane_exit_notification("%pane-exited %1"));
        assert!(is_pane_exit_notification("%pane-died %1"));
        assert!(!is_pane_exit_notification("%layout-change @1 layout"));
        assert_eq!(equal_sizes(80, 6), [13, 13, 13, 12, 12, 12]);
    }

    #[test]
    fn restores_mouse_and_focus_modes_for_existing_tmux_panes() {
        let modes =
            restored_terminal_modes(b"9\t4\t1\t1\t1\t0\t1\t0\t0\t1\t1,7,25,1002,1004,1006,2004")
                .unwrap();
        let text = String::from_utf8(modes).unwrap();
        assert!(text.starts_with("\x1b[5;10H\x1b[?25h\x1b[?1h\x1b[?2004h"));
        assert!(text.contains("\x1b[?1000l\x1b[?1002h\x1b[?1003l"));
        assert!(text.contains("\x1b[?1005l\x1b[?1006h\x1b[?1004h"));
        assert!(restored_terminal_modes(b"0\t0\t1\t0\t0").is_none());
    }

    #[test]
    fn snapshot_maps_native_session_window_and_pane_ids() {
        let draft = Draft {
            sessions: vec![b"$1\twork\t/home/dev/project".to_vec()],
            windows: vec![
                b"$1\t@4\t2\t1\t80\t24\tabcd,80x24,0,0{39x24,0,0,7,40x24,40,0,8}\teditor".to_vec(),
            ],
            panes: vec![
                b"%7\t@4\t0\t0\tbash\t".to_vec(),
                b"%8\t@4\t1\t1\tvim\tProject editor".to_vec(),
            ],
        };
        let s = parse_snapshot(&draft, "$1").unwrap();
        let w = s.window().unwrap();
        assert_eq!(s.session().unwrap().cwd, "/home/dev/project");
        assert_eq!(w.id, "@4");
        assert_eq!(w.active_pane, "%8");
        assert_eq!(w.panes[1].x, 40);
        assert_eq!(w.panes[0].title, "bash");
        assert_eq!(w.panes[1].title, "Project editor");
    }

    #[test]
    fn session_creation_expands_home_without_executing_path_text() {
        assert_eq!(shell_directory("~"), "\"$HOME\"");
        assert_eq!(
            shell_directory("~/project $(literal)"),
            "\"$HOME\"/'project $(literal)'"
        );
        let requests = action_requests(
            Action::NewSessionAt("relative/project".into()),
            &Snapshot::default(),
        )
        .unwrap();
        assert_eq!(requests[0].directory.as_deref(), Some("relative/project"));
        assert!(requests[0].command.contains("-c \"$PWD\""));
    }
    #[test]
    #[ignore = "requires TSHELL_SSH_TEST_HOST; isolated tmux UTF-8 metadata under C locale"]
    fn remote_metadata_preserves_chinese_in_c_locale() -> Result<()> {
        let config = HostConfig {
            destination: std::env::var("TSHELL_SSH_TEST_HOST")?,
            name: String::new(),
            user: String::new(),
            port: None,
            identity_file: None,
            tmux: true,
            socket: Some(format!(
                "tshell-unicode-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            )),
        };
        let tmux = config.tmux()?;
        let result = (|| -> Result<()> {
            crate::ssh_pool::output(
                &config,
                format!("LC_ALL=C {tmux} new-session -d -s '中文会话' -n '中文窗口'"),
                65536,
            )?;
            for _ in 0..3 {
                let output = crate::ssh_pool::output(
                    &config,
                    format!(
                        "LC_ALL=C {tmux} -C list-sessions -F '#{{session_name}}'; LC_ALL=C {tmux} -C list-windows -F '#{{window_name}}'"
                    ),
                    65536,
                )?;
                let events = ControlParser::default().push(&output)?;
                let lines: Vec<_> = events
                    .into_iter()
                    .filter_map(|event| match event {
                        ControlEvent::Response {
                            success: true,
                            lines,
                        } => Some(lines),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                assert!(lines.iter().any(|line| line == "中文会话".as_bytes()));
                assert!(lines.iter().any(|line| line == "中文窗口".as_bytes()));
                let snapshot = parse_snapshot(&discover(&config, false)?, "")?;
                assert_eq!(snapshot.sessions[0].name, "中文会话");
            }
            Ok(())
        })();
        let _ = crate::ssh_pool::output(&config, format!("{tmux} kill-server"), 65536);
        result
    }

    #[test]
    fn creating_session_quotes_remote_directory() {
        let requests = action_requests(
            Action::NewSessionAt("/home/dev/it's project".into()),
            &Snapshot::default(),
        )
        .unwrap();
        assert_eq!(
            requests[0].command,
            "new-session -d -c '/home/dev/it'\\''s project' -P -F '#{session_id}'"
        );
        let path = "/tmp/中文 ' \\ folder $(literal)";
        let requests =
            action_requests(Action::NewSessionAt(path.into()), &Snapshot::default()).unwrap();
        let args = shell_words::split(&requests[0].command).unwrap();
        assert_eq!(args[3], path);
    }

    #[test]
    fn closing_last_terminal_does_not_create_replacement() {
        let mut state = split_test_snapshot("%1");
        state.sessions[0].cwd = "/home/dev/project space".into();
        state.sessions[0].windows[0]
            .panes
            .retain(|pane| pane.id == "%1");
        for action in [Action::ClosePane, Action::CloseWindow] {
            let requests = action_requests(action, &state).unwrap();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].command.starts_with("kill-"));
            assert!(!requests[0].command.contains("new-window"));
        }
        state.sessions[0].windows[0].zoomed = true;
        let requests = action_requests(Action::ClosePane, &state).unwrap();
        assert!(requests[0].command.starts_with("kill-pane"));
    }

    #[test]
    fn zoom_does_not_rebalance_hidden_panes_as_exited() {
        let previous = split_test_snapshot("%1");
        let mut zoomed = previous.clone();
        let window = &mut zoomed.sessions[0].windows[0];
        window.zoomed = true;
        window.panes.truncate(1);
        assert!(rebalance_removed_panes(&previous, &zoomed).is_empty());
        let expected = zoomed.clone();
        normalize_removed_panes(&previous, &mut zoomed);
        assert_eq!(zoomed, expected);
    }

    fn split_test_snapshot(active_pane: &str) -> Snapshot {
        Snapshot {
            active_session: "$1".into(),
            sessions: vec![SessionInfo {
                id: "$1".into(),
                active_window: "@4".into(),
                windows: vec![WindowInfo {
                    id: "@4".into(),
                    cols: 120,
                    rows: 30,
                    active_pane: active_pane.into(),
                    panes: vec![
                        crate::backend::PaneInfo {
                            id: "%0".into(),
                            cols: 40,
                            rows: 30,
                            ..Default::default()
                        },
                        crate::backend::PaneInfo {
                            id: "%1".into(),
                            x: 41,
                            cols: 39,
                            rows: 14,
                            ..Default::default()
                        },
                        crate::backend::PaneInfo {
                            id: "%3".into(),
                            x: 41,
                            y: 15,
                            cols: 39,
                            rows: 15,
                            ..Default::default()
                        },
                        crate::backend::PaneInfo {
                            id: "%2".into(),
                            x: 81,
                            cols: 39,
                            rows: 30,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn rename_window_uses_the_window_id_and_validates_the_name() {
        let commands: Vec<_> = action_requests(
            Action::RenameWindow {
                id: "@4".into(),
                name: "Build logs".into(),
            },
            &split_test_snapshot("%3"),
        )
        .unwrap()
        .into_iter()
        .map(|request| request.command)
        .collect();
        assert_eq!(commands, ["rename-window -t '@4' 'Build logs'"]);
        assert!(
            action_requests(
                Action::RenameWindow {
                    id: "@4".into(),
                    name: "".into()
                },
                &split_test_snapshot("%3"),
            )
            .is_err()
        );
    }

    #[test]
    fn horizontal_split_adds_an_equal_full_height_column() {
        let requests = action_requests(
            Action::Split(SplitAxis::Horizontal),
            &split_test_snapshot("%3"),
        )
        .unwrap();
        let commands: Vec<_> = requests
            .into_iter()
            .map(|request| request.command)
            .collect();
        assert_eq!(
            commands,
            [
                "split-window -h -f -l 29 -t '%3' -c '#{pane_current_path}'",
                "resize-pane -x 30 -t '%0'",
                "resize-pane -x 29 -t '%1'",
                "resize-pane -x 29 -t '%2'",
            ]
        );
    }

    #[test]
    fn vertical_split_equalizes_only_the_active_column() {
        let requests = action_requests(
            Action::Split(SplitAxis::Vertical),
            &split_test_snapshot("%1"),
        )
        .unwrap();
        let commands: Vec<_> = requests
            .into_iter()
            .map(|request| request.command)
            .collect();
        assert_eq!(
            commands,
            [
                "split-window -v -l 9 -t '%1' -c '#{pane_current_path}'",
                "resize-pane -y 10 -t '%1'",
                "resize-pane -y 9 -t '%3'",
            ]
        );
    }

    #[test]
    fn closing_a_pane_rebalances_its_rows_or_columns() {
        let row_requests = action_requests(Action::ClosePane, &split_test_snapshot("%1")).unwrap();
        let row_commands: Vec<_> = row_requests
            .into_iter()
            .map(|request| request.command)
            .collect();
        assert_eq!(
            row_commands,
            ["kill-pane -t '%1' ; resize-pane -y 30 -t '%3'"]
        );

        let column_requests =
            action_requests(Action::ClosePane, &split_test_snapshot("%2")).unwrap();
        let column_commands: Vec<_> = column_requests
            .into_iter()
            .map(|request| request.command)
            .collect();
        assert_eq!(
            column_commands,
            ["kill-pane -t '%2' ; resize-pane -x 60 -t '%0' ; resize-pane -x 59 -t '%1'"]
        );

        let mut horizontal = split_test_snapshot("%1");
        let horizontal_window = &mut horizontal.sessions[0].windows[0];
        horizontal_window.panes.retain(|pane| pane.id != "%3");
        horizontal_window.panes[1].rows = 30;
        let horizontal_commands: Vec<_> = action_requests(Action::ClosePane, &horizontal)
            .unwrap()
            .into_iter()
            .map(|request| request.command)
            .collect();
        assert_eq!(
            horizontal_commands,
            ["kill-pane -t '%1' ; select-layout -t '@4' even-horizontal"]
        );
    }

    #[test]
    fn observed_pane_exit_rebalances_an_equal_layout() {
        let mut previous = split_test_snapshot("%1");
        let previous_window = &mut previous.sessions[0].windows[0];
        previous_window.panes.retain(|pane| pane.id != "%3");
        previous_window.panes[1].rows = 30;
        assert!(is_equal_column_layout(previous_window));

        let mut next = previous.clone();
        let next_window = &mut next.sessions[0].windows[0];
        next_window.panes.retain(|pane| pane.id != "%1");
        next_window.panes[0].cols = 80;
        next_window.panes[1].x = 81;
        next_window.panes[1].cols = 39;

        let commands: Vec<_> = rebalance_removed_panes(&previous, &next)
            .into_iter()
            .map(|request| request.command)
            .collect();
        assert_eq!(commands, ["select-layout -t '@4' even-horizontal"]);

        normalize_removed_panes(&previous, &mut next);
        let panes = &next.window().unwrap().panes;
        assert_eq!((panes[0].x, panes[0].cols), (0, 60));
        assert_eq!((panes[1].x, panes[1].cols), (61, 59));
    }

    #[test]
    #[ignore = "requires TSHELL_SSH_TEST_HOST; uses an isolated tmux socket"]
    fn real_tmux_equal_close_and_exit_stay_horizontal() {
        struct Cleanup(HostConfig);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Ok(mut command) = self.0.ssh_command() {
                    let _ = command
                        .arg(format!("{} kill-server", self.0.tmux().unwrap()))
                        .output();
                }
            }
        }
        let destination = std::env::var("TSHELL_SSH_TEST_HOST").expect("set test host");
        let config = HostConfig {
            destination,
            name: String::new(),
            user: String::new(),
            port: None,
            identity_file: None,
            tmux: true,

            socket: Some(format!(
                "tshell-layout-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            )),
        };
        let _cleanup = Cleanup(config.clone());
        let client = TmuxClient::connect(config);
        let wait = |pane_count: usize| {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let state = client.snapshot();
                if let Some(window) = state.window() {
                    let min = window.panes.iter().map(|pane| pane.cols).min().unwrap_or(0);
                    let max = window.panes.iter().map(|pane| pane.cols).max().unwrap_or(0);
                    if window.panes.len() == pane_count
                        && window
                            .panes
                            .iter()
                            .all(|pane| pane.y == 0 && pane.rows == window.rows)
                        && max - min <= 1
                    {
                        return state;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "layout did not settle: {state:?}"
                );
                thread::sleep(Duration::from_millis(20));
            }
        };
        wait(1);
        for pane_count in 2..=12 {
            client.apply(Action::Split(SplitAxis::Horizontal)).unwrap();
            wait(pane_count);
        }
        for pane_count in (2..12).rev() {
            client.apply(Action::ClosePane).unwrap();
            wait(pane_count);
        }

        for pane_count in 3..=12 {
            client.apply(Action::Split(SplitAxis::Horizontal)).unwrap();
            wait(pane_count);
        }
        for pane_count in (2..12).rev() {
            let state = client.snapshot();
            let exiting = state.window().unwrap().active_pane.clone();
            client
                .screen(&exiting)
                .unwrap()
                .input(b"exit\r".to_vec())
                .unwrap();
            wait(pane_count);
        }
    }

    #[test]
    #[ignore = "requires TSHELL_SSH_TEST_HOST; uses an isolated tmux socket"]
    fn real_ssh_tmux_workspace_round_trip() {
        struct Cleanup(HostConfig);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Ok(mut command) = self.0.ssh_command() {
                    let _ = command
                        .arg(format!("{} kill-server", self.0.tmux().unwrap()))
                        .output();
                }
            }
        }
        let destination = std::env::var("TSHELL_SSH_TEST_HOST").expect("set TSHELL_SSH_TEST_HOST");
        let config = HostConfig {
            destination,
            name: String::new(),
            user: String::new(),
            port: None,
            identity_file: None,
            tmux: true,

            socket: Some(format!(
                "tshell-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            )),
        };
        let cleanup = Cleanup(config.clone());
        let client = TmuxClient::connect(config.clone());
        let wait = |predicate: &dyn Fn(&Snapshot) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let state = client.snapshot();
                if predicate(&state) {
                    return state;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for tmux: {state:?}"
                );
                thread::sleep(Duration::from_millis(50));
            }
        };
        let initial = wait(&|s| s.connected() && s.window().is_some());
        assert_eq!(initial.session().unwrap().name, "tshell");
        // Consume titles through the notification path, not the calibration query.
        let notifications = client.updates.subscribe();
        let pane = initial.window().unwrap().active_pane.clone();
        client.screen(&pane).unwrap().input(
            b"for i in $(seq 1 20); do printf '\\033]2;TSHELL_LIVE_%s\\007' \"$i\"; sleep 0.05; done; sleep 1\r".to_vec()
        ).unwrap();
        let mut observed = BTreeSet::new();
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline && !observed.contains("TSHELL_LIVE_20") {
            if notifications.try_recv().is_ok() {
                let state = client.snapshot();
                for p in state
                    .sessions
                    .iter()
                    .flat_map(|s| &s.windows)
                    .flat_map(|w| &w.panes)
                {
                    if p.title.starts_with("TSHELL_LIVE_") {
                        observed.insert(p.title.clone());
                    }
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            observed.len() >= 8 && observed.contains("TSHELL_LIVE_20"),
            "title notifications too slow: {observed:?}"
        );
        println!("Received {} distinct live tmux titles", observed.len());
        let first_session = initial.active_session.clone();
        let first_window = initial.window().unwrap().id.clone();
        client
            .apply(Action::RenameSession {
                id: first_session.clone(),
                name: "demo-renamed".into(),
            })
            .unwrap();
        wait(&|s| s.session().is_some_and(|s| s.name == "demo-renamed"));
        client
            .screen(&pane)
            .unwrap()
            .input(b"cd /tmp\r".to_vec())
            .unwrap();
        wait(&|s| {
            s.window()
                .is_some_and(|w| w.panes.iter().any(|p| p.id == pane && p.cwd == "/tmp"))
        });
        client.resize(&first_window, 110, 32);
        wait(&|s| s.window().is_some_and(|w| w.cols == 110 && w.rows == 32));
        client.apply(Action::Split(SplitAxis::Horizontal)).unwrap();
        let split = wait(&|s| s.window().is_some_and(|w| w.panes.len() == 2));
        assert!(
            split
                .window()
                .unwrap()
                .panes
                .iter()
                .all(|p| p.cwd == "/tmp")
        );
        let active = split.window().unwrap().active_pane.clone();
        client
            .apply(Action::MoveFocus(crate::backend::Direction::Left))
            .unwrap();
        wait(&|s| s.window().is_some_and(|w| w.active_pane != active));
        client
            .apply(Action::MoveFocus(crate::backend::Direction::Right))
            .unwrap();
        wait(&|s| s.window().is_some_and(|w| w.active_pane == active));
        client
            .apply(Action::MovePane(crate::backend::Direction::Left))
            .unwrap();
        wait(&|s| {
            s.window()
                .is_some_and(|w| w.panes.iter().any(|p| p.id == active && p.x == 0))
        });
        client.apply(Action::Split(SplitAxis::Horizontal)).unwrap();
        let three = wait(&|s| {
            s.window().is_some_and(|w| {
                let min = w.panes.iter().map(|pane| pane.cols).min().unwrap_or(0);
                let max = w.panes.iter().map(|pane| pane.cols).max().unwrap_or(0);
                w.panes.len() == 3 && max - min <= 1
            })
        });
        let closing = three.window().unwrap().active_pane.clone();
        let closing_screen = client.screen(&closing).unwrap();
        closing_screen.input(b"exit\r".to_vec()).unwrap();
        wait(&|s| {
            s.window().is_some_and(|w| {
                let min = w.panes.iter().map(|pane| pane.cols).min().unwrap_or(0);
                let max = w.panes.iter().map(|pane| pane.cols).max().unwrap_or(0);
                w.panes.len() == 2 && max - min <= 1
            })
        });
        client.apply(Action::CycleLayout).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let screen = loop {
            if let Some(screen) = client.screen(&active) {
                break screen;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        };
        crate::terminal::check_remote_theme(&screen, |theme| client.set_theme(theme));
        screen
            .input(b"printf '\\033[31m%s%s\\033[0m\\n' TSHELL_ REMOTE_OK\r".to_vec())
            .unwrap();
        loop {
            let text: String = screen
                .term
                .lock()
                .grid()
                .display_iter()
                .map(|c| c.c)
                .collect();
            if text.contains("TSHELL_REMOTE_OK") {
                break;
            }
            assert!(Instant::now() < deadline, "remote output missing: {text}");
            thread::sleep(Duration::from_millis(30));
        }
        client.apply(Action::Zoom).unwrap();
        wait(&|s| s.window().is_some_and(|w| w.zoomed && w.panes.len() == 1));
        client.apply(Action::Zoom).unwrap();
        wait(&|s| s.window().is_some_and(|w| !w.zoomed && w.panes.len() == 2));
        client
            .apply(Action::NewWindowAt {
                path: "/".into(),
                name: "directory tab".into(),
            })
            .unwrap();
        wait(&|s| s.session().is_some_and(|s| s.windows.len() == 2));
        let created = wait(&|s| s.window().is_some_and(|w| w.name == "directory tab"));
        assert!(created.window().unwrap().panes.iter().all(|p| p.cwd == "/"));
        client
            .apply(Action::SelectWindow(first_window.clone()))
            .unwrap();
        wait(&|s| s.window().is_some_and(|w| w.id == first_window));
        let mut external = config.ssh_command().unwrap();
        assert!(
            external
                .arg(format!(
                    "{} new-window -d -t {} -n external-test",
                    config.tmux().unwrap(),
                    q(&format!("{first_session}:"))
                ))
                .status()
                .unwrap()
                .success()
        );
        wait(&|s| {
            s.session()
                .is_some_and(|s| s.windows.iter().any(|w| w.name == "external-test"))
        });
        client
            .apply(Action::NewNamedSession {
                path: "/tmp".into(),
                name: "second-session".into(),
            })
            .unwrap();
        let second =
            wait(&|s| s.sessions.len() == 2 && s.active_session != first_session).active_session;
        let created = client.snapshot();
        assert_eq!(created.session().unwrap().name, "second-session");
        assert_eq!(created.session().unwrap().cwd, "/tmp");
        let background_pane = client
            .snapshot()
            .sessions
            .iter()
            .find(|s| s.id == first_session)
            .unwrap()
            .windows
            .iter()
            .find(|w| w.id == first_window)
            .unwrap()
            .active_pane
            .clone();
        let background = client.screen(&background_pane).unwrap();
        background
            .input(b"printf '%s%s\\n' 'background-' 'alive'\r".to_vec())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let text: String = background
                .term
                .lock()
                .grid()
                .display_iter()
                .map(|c| c.c)
                .collect();
            if text.contains("background-alive") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "background session output missing: {text}"
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(client.snapshot().active_session, second);
        client
            .apply(Action::NewWindowInSession(second.clone()))
            .unwrap();
        wait(&|s| {
            s.session()
                .is_some_and(|s| s.id == second && s.windows.len() == 2)
        });
        client.apply(Action::RemoveSession(second)).unwrap();
        wait(&|s| s.sessions.len() == 1 && s.active_session == first_session);
        client.apply(Action::NewSession).unwrap();
        let pair = wait(&|s| s.sessions.len() == 2 && s.active_session != first_session);
        let fresh = pair.session().unwrap().clone();
        let clients = crate::ssh_pool::output(
            &config,
            format!(
                "{} list-clients -F '#{{session_id}}'",
                config.tmux().unwrap()
            ),
            65536,
        )
        .unwrap();
        let clients = String::from_utf8(clients).unwrap();
        for session in &pair.sessions {
            assert_eq!(
                clients.lines().filter(|id| *id == session.id).count(),
                1,
                "each session should have exactly one control client: {clients}"
            );
        }
        let survivor = client.screen(&background_pane).unwrap();
        client
            .screen(&fresh.windows[0].active_pane)
            .unwrap()
            .input(b"exit\r".to_vec())
            .unwrap();
        wait(&|s| s.sessions.len() == 1 && s.sessions[0].id == first_session);
        assert!(
            Arc::ptr_eq(&survivor, &client.screen(&background_pane).unwrap()),
            "another session was recreated during recovery"
        );
        client
            .apply(Action::SelectSession(first_session.clone()))
            .unwrap();
        wait(&|s| s.active_session == first_session);
        drop(client);
        // Verify disconnect did not destroy the native sessions.
        let output = config
            .ssh_command()
            .unwrap()
            .arg(format!(
                "{} list-sessions -F '#{{session_id}}'",
                config.tmux().unwrap()
            ))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);
        drop(screen);
        drop(cleanup);
    }
}
