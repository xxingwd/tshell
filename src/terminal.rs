//! Terminal state and PTY transport. No dependency on the UI framework.
mod directory;
mod ssh;
use crate::terminal_protocol::{OutputParser, ProtocolState, Replies, TerminalTheme};
use crate::terminal_snapshot::{RenderCommand, RenderSnapshot, apply_commands};
use alacritty_terminal::{
    event::{Event, EventListener},
    grid::Dimensions,
    term::{Config, Term, TermMode, cell::Cell},
    vte::ansi::{CursorShape, CursorStyle},
};
use anyhow::{Context, Result};
use parking_lot::{Condvar, Mutex};
use portable_pty::{ChildKiller, CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
};

fn local_command(cwd: &PathBuf) -> CommandBuilder {
    let mut command = if cfg!(windows) {
        let mut command = CommandBuilder::new("powershell.exe");
        command.args(["-NoLogo", "-NoProfile", "-NoExit", "-Command",
            "function global:prompt { $u = [System.Uri]::new((Get-Location).Path + [IO.Path]::DirectorySeparatorChar); [Console]::Write(([char]27).ToString() + ']7;' + $u.AbsoluteUri + [char]7); 'PS ' + $executionContext.SessionState.Path.CurrentLocation + ('>' * ($nestedPromptLevel + 1)) + ' ' }"]);

        command
    } else {
        let mut command = CommandBuilder::new(std::env::var("SHELL").unwrap_or("/bin/bash".into()));
        command.arg("-l");
        command
    };
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "TShell");
    command
}

pub fn validate_destination(value: &str) -> Result<String> {
    let value = value.trim();
    anyhow::ensure!(
        !value.is_empty(),
        "{}",
        crate::t!("term.destination_required")
    );
    anyhow::ensure!(
        !value.starts_with('-') && !value.chars().any(|c| c.is_whitespace() || c.is_control()),
        "{}",
        crate::t!("term.destination_whitespace")
    );
    Ok(value.to_owned())
}

enum Command {
    Input(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
        generation: u64,
    },
    Stop,
}

#[derive(Default)]
struct ResizeState {
    requested: u64,
    completed: u64,
    failed: bool,
}

/// Keeps user input behind the latest PTY resize without blocking the UI thread.
/// The transport acknowledges the resize; the bounded wait is only a failure
/// fallback for a transport that closes while a resize is pending.
#[derive(Default)]
struct ResizeBarrier {
    state: Mutex<ResizeState>,
    wake: Condvar,
}

impl ResizeBarrier {
    fn enqueue(&self, rows: u16, cols: u16, send: impl FnOnce(Command) -> bool) -> bool {
        let mut state = self.state.lock();
        let generation = state.requested.saturating_add(1);
        if !send(Command::Resize {
            rows,
            cols,
            generation,
        }) {
            return false;
        }
        state.requested = generation;
        true
    }

    fn complete(&self, generation: u64) {
        let mut state = self.state.lock();
        state.completed = state.completed.max(generation);
        self.wake.notify_all();
    }

    fn fail(&self) {
        let mut state = self.state.lock();
        state.failed = true;
        self.wake.notify_all();
    }

    fn wait_for_latest(&self) {
        const RESIZE_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);
        loop {
            let mut state = self.state.lock();
            let target = state.requested;
            while !state.failed && state.completed < target {
                if self
                    .wake
                    .wait_for(&mut state, RESIZE_ACK_TIMEOUT)
                    .timed_out()
                {
                    return;
                }
            }
            let stable = state.failed || state.requested == target;
            drop(state);
            if stable {
                return;
            }
        }
    }
}

#[derive(Clone)]
pub struct Listener {
    replies: Replies,
    title: Arc<Mutex<String>>,
    metadata: OutputWake,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            event @ (Event::PtyWrite(_) | Event::ColorRequest(_, _)) => {
                let mut replies = self.replies.lock();
                if replies.len() < 1024 {
                    replies.push_back(event);
                }
            }
            Event::Title(title) => {
                let mut current = self.title.lock();
                if *current != title {
                    *current = title;
                    self.metadata.notify();
                }
            }
            Event::ResetTitle => {
                let mut title = self.title.lock();
                if !title.is_empty() {
                    title.clear();
                    self.metadata.notify();
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
pub struct Size {
    pub rows: usize,
    pub cols: usize,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

fn terminal_config() -> Config {
    Config {
        scrolling_history: 10_000,
        default_cursor_style: CursorStyle {
            shape: CursorShape::Block,
            blinking: true,
        },
        ..Config::default()
    }
}

#[derive(Clone, Default)]
pub struct OutputWake(Arc<Mutex<Vec<async_channel::Sender<()>>>>);
impl OutputWake {
    pub fn subscribe(&self) -> async_channel::Receiver<()> {
        let (tx, rx) = async_channel::bounded(1);
        self.listen(tx);
        rx
    }
    pub(crate) fn listen(&self, tx: async_channel::Sender<()>) {
        let mut listeners = self.0.lock();
        listeners.retain(|listener| !listener.is_closed());
        if !listeners.iter().any(|listener| listener.same_channel(&tx)) {
            listeners.push(tx);
        }
    }
    pub(crate) fn notify(&self) {
        self.0.lock().retain(|tx| {
            let _ = tx.try_send(());
            !tx.is_closed()
        });
    }
}

pub struct Session {
    directory: Arc<Mutex<directory::Directory>>,
    render_snapshot: Mutex<Arc<RenderSnapshot>>,
    mode: Arc<AtomicU32>,
    pub updates: OutputWake,
    pub metadata: OutputWake,
    pub term: Arc<Mutex<Term<Listener>>>,
    pub title: Arc<Mutex<String>>,
    pub revision: Arc<AtomicU64>,
    pub exited: Arc<AtomicBool>,
    pub(crate) ready: AtomicBool,
    pub bytes_read: Arc<AtomicU64>,
    pub notice_count: Arc<AtomicU64>,
    pub error: Arc<Mutex<Option<String>>>,
    resize_barrier: Arc<ResizeBarrier>,
    tx: Option<SyncSender<Command>>,
    killer: Option<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    remote_input: Option<Arc<dyn Fn(Vec<u8>) -> Result<()> + Send + Sync>>,
    ssh_commands: Option<async_channel::Sender<Command>>,
    remote_parser: Mutex<OutputParser>,
    protocol: Arc<Mutex<ProtocolState>>,
    replies: Replies,
}

impl Session {
    pub fn start(cwd: PathBuf) -> Result<Self> {
        Self::spawn(local_command(&cwd))
    }

    pub fn spawn(command: CommandBuilder) -> Result<Self> {
        #[cfg(all(windows, target_arch = "x86_64"))]
        crate::windows_runtime::prepare()?;
        let pair = native_pty_system().openpty(PtySize {
            rows: 28,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut child = pair
            .slave
            .spawn_command(command)
            .context(crate::t!("term.spawn_failed"))?;
        drop(pair.slave);
        let killer = Mutex::new(child.clone_killer());
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let master = pair.master;
        let (tx, rx) = mpsc::sync_channel(256);
        let title = Arc::new(Mutex::new(String::new()));
        let metadata = OutputWake::default();
        let revision = Arc::new(AtomicU64::new(1));
        let exited = Arc::new(AtomicBool::new(false));
        let bytes_read = Arc::new(AtomicU64::new(0));
        let notice_count = Arc::new(AtomicU64::new(0));
        let read_notices = notice_count.clone();
        let error = Arc::new(Mutex::new(None));
        let protocol = Arc::new(Mutex::new(ProtocolState::default()));
        let replies = Replies::default();
        let term = Arc::new(Mutex::new(Term::new(
            terminal_config(),
            &Size {
                rows: 28,
                cols: 100,
            },
            Listener {
                replies: replies.clone(),
                title: title.clone(),
                metadata: metadata.clone(),
            },
        )));
        let updates = OutputWake::default();
        let mode = Arc::new(AtomicU32::new(term.lock().mode().bits()));
        let resize_barrier = Arc::new(ResizeBarrier::default());
        let read_mode = mode.clone();
        let write_updates = updates.clone();
        let write_error = error.clone();
        let write_metadata = metadata.clone();
        let write_resize = resize_barrier.clone();
        thread::Builder::new()
            .name("pty-write".into())
            .spawn(move || {
                while let Ok(command) = rx.recv() {
                    let result = match command {
                        Command::Input(data) => writer
                            .write_all(&data)
                            .and_then(|_| writer.flush())
                            .map_err(anyhow::Error::from),
                        Command::Resize {
                            rows,
                            cols,
                            generation,
                        } => {
                            let result = master.resize(PtySize {
                                rows,
                                cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            });
                            if result.is_ok() {
                                write_resize.complete(generation);
                            } else {
                                write_resize.fail();
                            }
                            result
                        }
                        Command::Stop => break,
                    };
                    if let Err(err) = result {
                        write_resize.fail();
                        *write_error.lock() = Some(err.to_string());
                        write_updates.notify();
                        write_metadata.notify();
                        break;
                    }
                }
            })?;
        let directory = Arc::new(Mutex::new(directory::Directory::default()));
        let read_directory = directory.clone();
        let read_updates = updates.clone();
        let read_metadata = metadata.clone();
        let read_protocol = protocol.clone();
        let read_replies = replies.clone();
        let reply_tx = tx.clone();
        let (read_term, read_rev, read_bytes, read_error) = (
            term.clone(),
            revision.clone(),
            bytes_read.clone(),
            error.clone(),
        );
        let read_resize = resize_barrier.clone();
        thread::Builder::new()
            .name("pty-read".into())
            .spawn(move || {
                let mut parser = OutputParser::default();
                let mut notifications = crate::terminal_notifications::Notifications::default();
                let mut clipboard = crate::terminal_clipboard::Clipboard::default();
                let mut buffer = [0u8; 32 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            clipboard.advance(&buffer[..count]);
                            if read_directory.lock().advance(&buffer[..count]) {
                                read_metadata.notify();
                            }
                            let notices = notifications.advance(&buffer[..count]);
                            if notices > 0 {
                                read_notices.fetch_add(notices, Ordering::Release);
                                read_metadata.notify();
                            }
                            let mut term = read_term.lock();
                            let output = parser.advance(
                                &mut *term,
                                &buffer[..count],
                                &mut read_protocol.lock(),
                                &read_replies,
                                true,
                            );
                            read_mode.store(term.mode().bits(), Ordering::Release);
                            read_bytes.fetch_add(count as u64, Ordering::Relaxed);
                            if output.visual {
                                read_rev.fetch_add(1, Ordering::Release);
                            }
                            drop(term);
                            for response in output.replies {
                                if reply_tx.send(Command::Input(response)).is_err() {
                                    break;
                                }
                            }
                            if output.visual {
                                read_updates.notify();
                            }
                        }
                        Err(err) => {
                            // Windows reports a broken pipe when its PTY closes normally.
                            if err.kind() != std::io::ErrorKind::BrokenPipe
                                && err.raw_os_error() != Some(5)
                            {
                                *read_error.lock() = Some(err.to_string());
                            }
                            break;
                        }
                    }
                }
                read_resize.fail();
                read_updates.notify();
                read_metadata.notify();
            })?;
        let wait_updates = updates.clone();
        let wait_metadata = metadata.clone();
        let (wait_exit, wait_rev) = (exited.clone(), revision.clone());
        thread::Builder::new()
            .name("pty-wait".into())
            .spawn(move || {
                let _ = child.wait();
                wait_exit.store(true, Ordering::Release);
                wait_rev.fetch_add(1, Ordering::Release);
                wait_updates.notify();
                wait_metadata.notify();
            })?;
        Ok(Self {
            directory,
            mode,
            render_snapshot: Mutex::new(Arc::new(RenderSnapshot::default())),
            updates,
            metadata,
            term,
            title,
            revision,
            exited,
            ready: AtomicBool::new(true),
            bytes_read,
            notice_count,
            error,
            resize_barrier,
            tx: Some(tx),
            killer: Some(killer),
            remote_input: None,
            ssh_commands: None,
            remote_parser: Mutex::new(OutputParser::default()),
            protocol,
            replies,
        })
    }

    pub fn remote(
        id: String,
        rows: usize,
        cols: usize,
        input: Arc<dyn Fn(Vec<u8>) -> Result<()> + Send + Sync>,
    ) -> Arc<Self> {
        let title = Arc::new(Mutex::new(id.clone()));
        let metadata = OutputWake::default();
        let revision = Arc::new(AtomicU64::new(1));
        let protocol = Arc::new(Mutex::new(ProtocolState::default()));
        let replies = Replies::default();
        let term = Arc::new(Mutex::new(Term::new(
            terminal_config(),
            &Size { rows, cols },
            Listener {
                replies: replies.clone(),
                title: title.clone(),
                metadata: metadata.clone(),
            },
        )));
        let mode = Arc::new(AtomicU32::new(term.lock().mode().bits()));
        Arc::new(Self {
            directory: Arc::new(Mutex::new(directory::Directory::default())),
            mode,
            render_snapshot: Mutex::new(Arc::new(RenderSnapshot::default())),
            updates: OutputWake::default(),
            metadata,
            term,
            title,
            revision,
            exited: Arc::new(AtomicBool::new(false)),
            ready: AtomicBool::new(true),
            bytes_read: Arc::new(AtomicU64::new(0)),
            notice_count: Arc::new(AtomicU64::new(0)),
            error: Arc::new(Mutex::new(None)),
            resize_barrier: Arc::new(ResizeBarrier::default()),
            tx: None,
            killer: None,
            remote_input: Some(input),
            ssh_commands: None,
            remote_parser: Mutex::new(OutputParser::default()),
            protocol,
            replies,
        })
    }

    pub fn current_directory(&self) -> Option<String> {
        self.directory.lock().path.clone()
    }
    pub fn set_initial_directory(&self, path: Option<String>) {
        let mut directory = self.directory.lock();
        if directory.path.is_none() {
            directory.path = path;
        }
    }
    pub fn remote_output(&self, bytes: &[u8]) {
        if self.directory.lock().advance(bytes) {
            self.metadata.notify();
        }
        let mut parser = self.remote_parser.lock();
        let mut term = self.term.lock();
        let output = parser.advance(
            &mut *term,
            bytes,
            &mut self.protocol.lock(),
            &self.replies,
            self.ssh_commands.is_some(),
        );
        self.mode.store(term.mode().bits(), Ordering::Release);
        self.bytes_read
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        if output.visual {
            self.revision.fetch_add(1, Ordering::Release);
        }
        drop(term);
        drop(parser);
        for reply in output.replies {
            if let Err(error) = self.send_input(reply) {
                *self.error.lock() = Some(error.to_string());
            }
        }
        if output.visual {
            self.updates.notify();
        }
    }

    pub fn remote_observe(&self, bytes: &[u8]) {
        self.remote_parser
            .lock()
            .observe(bytes, &mut self.protocol.lock());
    }

    /// tmux capture/mode restoration is synthetic output, not an application
    /// resetting or subscribing to terminal reports. Do not change the probe.
    pub fn remote_restore(&self, bytes: &[u8]) {
        let mut term = self.term.lock();
        let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
        parser.advance(&mut *term, bytes);
        self.replies.lock().clear();
        self.mode.store(term.mode().bits(), Ordering::Release);
        self.revision.fetch_add(1, Ordering::Release);
        drop(term);
        self.updates.notify();
    }

    /// tmux capture carries SGR state across lines. Even with -N, unallocated
    /// trailing cells are absent; synthetic line feeds must not fill scrolled
    /// blank rows with the previous line's cell attributes.
    pub fn remote_restore_capture(&self, lines: &[Vec<u8>], alternate: bool) {
        let mut term = self.term.lock();
        let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
        parser.advance(&mut *term, b"\x1bc");
        if alternate {
            parser.advance(&mut *term, b"\x1b[?1049h");
        }
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                let template =
                    std::mem::replace(&mut term.grid_mut().cursor.template, Cell::default());
                parser.advance(&mut *term, b"\r\n");
                term.grid_mut().cursor.template = template;
            }
            parser.advance(&mut *term, line);
        }
        self.replies.lock().clear();
        self.mode.store(term.mode().bits(), Ordering::Release);
        self.revision.fetch_add(1, Ordering::Release);
        drop(term);
        self.updates.notify();
    }

    /// Run on the background executor. Serializes damage consumption and keeps the
    /// previous immutable rows alive for any view still painting an older frame.
    pub(crate) fn render_snapshot(
        &self,
        commands: std::collections::VecDeque<RenderCommand>,
    ) -> (Arc<RenderSnapshot>, Vec<String>) {
        let copies = self.apply_interactions(commands);
        let mut previous = self.render_snapshot.lock();
        let mut term = self.term.lock();
        let revision = self.revision.load(Ordering::Acquire);
        let next = Arc::new(RenderSnapshot::capture(&mut term, revision, &previous));
        *previous = next.clone();
        drop(term);
        drop(previous);
        (next, copies)
    }

    /// The view serializes interaction batches independently of snapshot work.
    pub(crate) fn apply_interactions(
        &self,
        commands: std::collections::VecDeque<RenderCommand>,
    ) -> Vec<String> {
        if commands.is_empty() {
            return Vec::new();
        }
        let (copies, inputs) = apply_commands(&mut self.term.lock(), commands);
        for data in inputs {
            if let Err(error) = self.input(data) {
                *self.error.lock() = Some(error.to_string());
                self.updates.notify();
                break;
            }
        }
        copies
    }

    pub fn remote_resize(&self, rows: usize, cols: usize) {
        let mut term = self.term.lock();
        term.resize(Size {
            rows: rows.max(2),
            cols: cols.max(2),
        });
        self.revision.fetch_add(1, Ordering::Release);
        drop(term);
        self.updates.notify();
    }

    pub fn is_remote(&self) -> bool {
        self.remote_input.is_some()
    }

    /// Input encoding follows the latest parsed modes, independently of frame lag.
    pub(crate) fn mode(&self) -> TermMode {
        TermMode::from_bits_retain(self.mode.load(Ordering::Acquire))
    }
    pub fn set_theme(&self, theme: TerminalTheme) {
        let notify = {
            let mut state = self.protocol.lock();
            let notify = state.subscribed && state.theme.light() != theme.light();
            state.theme = theme;
            notify
        };
        if notify {
            // Resize and protocol reports share the transport queue; theme changes run on the UI thread.
            if let Err(error) = self.send_input(theme.report()) {
                *self.error.lock() = Some(error.to_string());
                self.updates.notify();
            }
        }
    }

    pub fn input(&self, data: Vec<u8>) -> Result<()> {
        anyhow::ensure!(
            data.len() <= 1024 * 1024,
            "{}",
            crate::t!("term.paste_too_large")
        );
        self.resize_barrier.wait_for_latest();
        self.send_input(data)
    }

    fn send_input(&self, data: Vec<u8>) -> Result<()> {
        if let Some(input) = &self.remote_input {
            return input(data);
        }
        self.tx
            .as_ref()
            .context(crate::t!("term.closed"))?
            .try_send(Command::Input(data))
            .context(crate::t!("term.queue_full"))
    }

    pub fn resize(&self, rows: usize, cols: usize) {
        if let Some(sender) = &self.ssh_commands {
            let (rows, cols) = (rows.clamp(2, 500), cols.clamp(2, 1000));
            let term = self.term.lock();
            let changed = term.screen_lines() != rows || term.columns() != cols;
            drop(term);
            if changed {
                if self
                    .resize_barrier
                    .enqueue(rows as u16, cols as u16, |command| {
                        sender.try_send(command).is_ok()
                    })
                {
                    self.remote_resize(rows, cols);
                }
            }
            return;
        }
        if self.is_remote() {
            return;
        }
        let (rows, cols) = (rows.clamp(2, 500), cols.clamp(2, 1000));
        let mut term = self.term.lock();
        if term.screen_lines() != rows || term.columns() != cols {
            let Some(tx) = self.tx.as_ref() else {
                return;
            };
            if self
                .resize_barrier
                .enqueue(rows as u16, cols as u16, |command| {
                    tx.try_send(command).is_ok()
                })
            {
                term.resize(Size { rows, cols });
                self.revision.fetch_add(1, Ordering::Release);
                self.updates.notify();
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(sender) = &self.ssh_commands {
            sender.close();
        }
        if let Some(killer) = &self.killer {
            let _ = killer.lock().kill();
        }
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(Command::Stop);
        }
    }
}

#[cfg(test)]
pub(crate) fn check_remote_theme(session: &Session, change_theme: impl Fn(TerminalTheme)) {
    use std::time::{Duration, Instant};
    let base = crate::terminal_protocol::default_theme();
    change_theme(TerminalTheme {
        background: 0xffffff,
        foreground: 0x273244,
        ..base
    });
    thread::sleep(Duration::from_millis(600));
    let script = r#"import os,sys,tty,termios,select,time
fd=sys.stdin.fileno()
old=termios.tcgetattr(fd)
tty.setraw(fd)
data=b''
try:
 os.write(1,b'\x1b]11;?\x07\x1b[?2031h')
 print('THEME'+'-READY',flush=True)
 until=time.monotonic()+4
 while time.monotonic()<until:
  if select.select([fd],[],[],0.1)[0]: data+=os.read(fd,4096)
 os.write(1,b'\x1b[?2031l')
finally:
 termios.tcsetattr(fd,termios.TCSANOW,old)
print('THEME'+'-RESULT:'+data.hex(),flush=True)
"#;
    session
        .input(format!("python3 -c '{}'\r", script.replace('\'', "'\\''")).into_bytes())
        .unwrap();
    let text = || -> String {
        session
            .term
            .lock()
            .grid()
            .display_iter()
            .map(|c| c.c)
            .collect()
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while !text().contains("THEME-READY") {
        assert!(Instant::now() < deadline, "probe did not start: {}", text());
        thread::sleep(Duration::from_millis(25));
    }
    thread::sleep(Duration::from_millis(150));
    change_theme(TerminalTheme {
        background: 0x101622,
        ..base
    });
    while !text().contains("THEME-RESULT:") {
        assert!(
            Instant::now() < deadline,
            "probe did not finish: {}",
            text()
        );
        thread::sleep(Duration::from_millis(25));
    }
    let result = text();
    assert!(
        result.contains("1b5d31313b7267623a666666662f666666662f66666666"),
        "background query not white: {result}"
    );
    assert!(
        result.contains("1b5b3f3939373b316e"),
        "dark theme notification missing: {result}"
    );
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::vte::ansi::{Color, NamedColor};
    #[test]
    fn interactions_do_not_wait_for_snapshot_serialization() {
        let sent = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let output = sent.clone();
        let session = super::Session::remote(
            "input-test".into(),
            4,
            20,
            std::sync::Arc::new(move |bytes| {
                output.lock().push(bytes);
                Ok(())
            }),
        );
        let _snapshot_guard = session.render_snapshot.lock();
        session.apply_interactions(std::collections::VecDeque::from([
            crate::terminal_snapshot::RenderCommand::Input(b"one".to_vec()),
            crate::terminal_snapshot::RenderCommand::Input(b"two".to_vec()),
        ]));
        assert_eq!(*sent.lock(), [b"one".to_vec(), b"two".to_vec()]);
    }

    use super::*;

    #[test]
    fn resize_barrier_tracks_latest_generation_and_failure() {
        let barrier = ResizeBarrier::default();
        let (sender, _receiver) = async_channel::bounded(2);
        assert!(barrier.enqueue(24, 80, |command| sender.try_send(command).is_ok()));
        let first = barrier.state.lock().requested;
        assert!(barrier.enqueue(25, 80, |command| sender.try_send(command).is_ok()));
        let latest = barrier.state.lock().requested;
        assert_eq!(first + 1, latest);

        barrier.complete(first);
        {
            let state = barrier.state.lock();
            assert_eq!(state.requested, latest);
            assert_eq!(state.completed, first);
            assert!(!state.failed);
        }

        barrier.fail();
        barrier.wait_for_latest();
        assert!(barrier.state.lock().failed);
    }

    #[test]
    fn full_resize_queue_does_not_disable_later_barriers() {
        let barrier = Arc::new(ResizeBarrier::default());
        let (sender, receiver) = async_channel::bounded(1);
        assert!(barrier.enqueue(24, 80, |command| sender.try_send(command).is_ok()));
        assert!(!barrier.enqueue(25, 80, |command| sender.try_send(command).is_ok()));
        assert_eq!(barrier.state.lock().requested, 1);
        receiver.try_recv().unwrap();
        barrier.complete(1);
        assert!(barrier.enqueue(26, 80, |command| sender.try_send(command).is_ok()));

        let (done, finished) = std::sync::mpsc::sync_channel(1);
        let waiting = barrier.clone();
        let worker = std::thread::spawn(move || {
            waiting.wait_for_latest();
            done.send(()).unwrap();
        });
        assert!(
            finished
                .recv_timeout(std::time::Duration::from_millis(20))
                .is_err()
        );
        barrier.complete(2);
        finished
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn theme_report_does_not_wait_for_pending_resize() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let output = sent.clone();
        let session = Session::remote(
            "%theme-resize".into(),
            24,
            80,
            Arc::new(move |data| {
                output.lock().push(data);
                Ok(())
            }),
        );
        let previous = session.protocol.lock().theme;
        session.protocol.lock().subscribed = true;
        assert!(session.resize_barrier.enqueue(25, 80, |_| true));

        let theme = TerminalTheme {
            background: if previous.light() { 0x101010 } else { 0xf0f0f0 },
            ..previous
        };
        let started = std::time::Instant::now();
        session.set_theme(theme);
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        assert_eq!(*sent.lock(), vec![theme.report()]);
    }

    #[test]
    fn osc_zero_and_two_update_the_terminal_title() {
        let session = Session::remote("%title".into(), 24, 80, Arc::new(|_| Ok(())));
        session.remote_restore(b"\x1b]2;Build logs\x07");
        assert_eq!(&*session.title.lock(), "Build logs");
        session.remote_restore(b"\x1b]0;Project API\x1b\\");
        assert_eq!(&*session.title.lock(), "Project API");
    }

    #[test]
    fn tmux_capture_scroll_keeps_empty_rows_default_without_losing_sgr() {
        use alacritty_terminal::{
            index::{Column, Line},
            term::cell::Flags,
        };

        let session = Session::remote("%capture".into(), 3, 8, Arc::new(|_| Ok(())));
        session.remote_resize(4, 8);
        session.remote_restore_capture(
            &[
                b"\x1b[31;7;46mX".to_vec(),
                Vec::new(),
                b"\x1b[46mA  ".to_vec(),
                b"B".to_vec(),
                b"\x1b[49mC".to_vec(),
            ],
            true,
        );

        let term = session.term.lock();
        assert!(term.mode().contains(TermMode::ALT_SCREEN));
        let grid = term.grid();
        let default = Color::Named(NamedColor::Background);
        let cyan = Color::Named(NamedColor::Cyan);
        assert!((0..8).all(|col| grid[Line(0)][Column(col)].bg == default));
        assert_eq!(
            grid[Line(0)][Column(0)].fg,
            Color::Named(NamedColor::Foreground)
        );
        assert!(!grid[Line(0)][Column(0)].flags.contains(Flags::INVERSE));
        assert_eq!(grid[Line(1)][Column(0)].c, 'A');
        assert_eq!(grid[Line(1)][Column(1)].bg, cyan);
        assert_eq!(grid[Line(1)][Column(2)].bg, cyan);
        assert_eq!(grid[Line(1)][Column(3)].bg, default);
        assert_eq!(grid[Line(2)][Column(0)].c, 'B');
        assert_eq!(grid[Line(2)][Column(0)].bg, cyan);
        assert_eq!(grid[Line(3)][Column(0)].c, 'C');
        assert_eq!(grid[Line(3)][Column(0)].bg, default);
    }

    #[test]
    fn frequent_titles_only_publish_the_latest_without_redrawing_the_grid() {
        use std::sync::atomic::Ordering;

        let session = Session::remote("%title-only".into(), 24, 80, Arc::new(|_| Ok(())));
        let (before, _) = session.render_snapshot(Default::default());
        let updates = session.updates.subscribe();
        let revision = session.revision.load(Ordering::Acquire);
        let metadata = session.metadata.subscribe();
        session.remote_output(b"\x1b]2;fragment");
        assert!(metadata.try_recv().is_err());
        session.remote_output(b"ed\x1b\\");
        for index in 0..1000 {
            session.remote_output(format!("\x1b]0;Build {index}\x07").as_bytes());
        }
        assert_eq!(&*session.title.lock(), "Build 999");
        assert_eq!(session.revision.load(Ordering::Acquire), revision);
        assert!(updates.try_recv().is_err());
        assert_eq!(metadata.len(), 1);
        metadata.try_recv().unwrap();
        session.remote_output(b"\x1b]2;Build 999\x07");
        assert!(metadata.try_recv().is_err());

        let (after, _) = session.render_snapshot(Default::default());
        assert!(before.same_visual(&after));
        assert_eq!(after.copied_rows, 0);

        session.remote_output(b"\x1b]2;Done\x07visible");
        assert_eq!(&*session.title.lock(), "Done");
        assert_eq!(session.revision.load(Ordering::Acquire), revision + 1);
        assert!(updates.try_recv().is_ok());
    }

    #[test]
    fn non_title_osc_still_notifies_the_renderer() {
        use std::sync::atomic::Ordering;

        let session = Session::remote("%colors".into(), 24, 80, Arc::new(|_| Ok(())));
        let updates = session.updates.subscribe();
        let revision = session.revision.load(Ordering::Acquire);
        session.remote_output(b"\x1b]11;#112233\x07");
        assert_eq!(session.revision.load(Ordering::Acquire), revision + 1);
        assert!(updates.try_recv().is_ok());
    }

    #[test]
    #[ignore = "requires TSHELL_SSH_TEST_HOST and python3"]
    fn real_ssh_theme_query_and_notification() {
        let host = std::env::var("TSHELL_SSH_TEST_HOST").unwrap();
        let session = Session::ssh(std::env::current_dir().unwrap(), &host, None).unwrap();
        thread::sleep(std::time::Duration::from_secs(2));
        check_remote_theme(&session, |theme| session.set_theme(theme));
    }
    #[test]
    fn color_queries_preserve_order_and_theme_notifications_are_opt_in() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sink = sent.clone();
        let session = Session::remote(
            "%protocol".into(),
            24,
            80,
            Arc::new(move |bytes| {
                sink.lock().push(bytes);
                Ok(())
            }),
        );
        let dark = crate::terminal_protocol::default_theme();
        let light = TerminalTheme {
            background: 0xffffff,
            foreground: 0x273244,
            ..dark
        };
        session.set_theme(light);
        let feed = |bytes: &[u8]| {
            session
                .remote_parser
                .lock()
                .advance(
                    &mut session.term.lock(),
                    bytes,
                    &mut session.protocol.lock(),
                    &session.replies,
                    true,
                )
                .replies
        };
        let mut replies = Vec::new();
        for byte in b"\x1b]11;#112233\x07\x1b]11;?\x07\x1b]111\x07\x1b]11;?\x1b\\\x1b]10;?\x07" {
            replies.extend(feed(&[*byte]));
        }
        assert_eq!(
            replies,
            [
                b"\x1b]11;rgb:1111/2222/3333\x07".to_vec(),
                b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\".to_vec(),
                b"\x1b]10;rgb:2727/3232/4444\x07".to_vec(),
            ]
        );
        assert_eq!(
            feed(b"\x1b[?2031$p\x1b[?996n"),
            [b"\x1b[?2031;2$y".to_vec(), b"\x1b[?997;2n".to_vec()]
        );
        feed(b"\x1b[?2031h");
        session.set_theme(dark);
        session.set_theme(dark);
        assert_eq!(*sent.lock(), [b"\x1b[?997;1n".to_vec()]);
        assert_eq!(feed(b"\x1b[?2031$p"), [b"\x1b[?2031;1$y".to_vec()]);
        feed(b"\x1b[?2031l");
        session.set_theme(light);
        assert_eq!(sent.lock().len(), 1);
        // The tmux server owns query replies. Merely observing its pane stream
        // must not inject duplicate colour/DSR answers into a running program.
        session.remote_output(b"\x1b]11;?\x07\x1b[?996n\x1b[6n");
        assert_eq!(sent.lock().len(), 1);
        // A tmux capture must not clear a live subscription or consume a
        // fragmented sequence still arriving on its control connection.
        session.remote_observe(b"\x1b[?203");
        session.remote_restore(b"\x1bcold capture");
        session.remote_observe(b"1h");
        session.remote_restore(b"\x1bcnew capture");
        session.set_theme(dark);
        assert_eq!(sent.lock().len(), 2);
        session.remote_output(b"\x1bc");
        session.set_theme(light);
        assert_eq!(sent.lock().len(), 2);
    }
    #[test]
    #[ignore = "interactive local shell latency measurement"]
    fn interactive_input_order_and_latency() {
        let session = Session::start(std::env::current_dir().unwrap()).unwrap();
        let text = || -> String {
            session
                .term
                .lock()
                .grid()
                .display_iter()
                .map(|c| c.c)
                .collect()
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while session.bytes_read.load(Ordering::Acquire) == 0 {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(1));
        }
        let mut typed = String::new();
        let mut samples = Vec::new();
        for ch in "#TSHELL_INPUT_ORDER_0123456789".chars() {
            typed.push(ch);
            let start = std::time::Instant::now();
            session.input(vec![ch as u8]).unwrap();
            while !text().contains(&typed) {
                assert!(
                    start.elapsed() < std::time::Duration::from_secs(3),
                    "input lost: {typed}"
                );
                thread::sleep(std::time::Duration::from_millis(1));
            }
            samples.push(start.elapsed().as_micros());
        }
        session.input(b"\r".to_vec()).unwrap();
        samples.sort();
        println!(
            "Local input to terminal grid: median={}us, max={}us, samples={}",
            samples[samples.len() / 2],
            samples.last().unwrap(),
            samples.len()
        );
    }

    #[test]
    fn output_notifications_are_bounded_and_do_not_drop_terminal_data() {
        let session = Session::remote("%test".into(), 24, 80, Arc::new(|_| Ok(())));
        let updates = session.updates.subscribe();
        for _ in 0..100 {
            session.remote_output(b"x");
        }
        assert_eq!(updates.len(), 1);
        updates.try_recv().unwrap();
        assert!(updates.try_recv().is_err());
        assert_eq!(session.bytes_read.load(Ordering::Acquire), 100);
        session.remote_output(b"y");
        updates.try_recv().unwrap();
        let text: String = session
            .term
            .lock()
            .grid()
            .display_iter()
            .map(|c| c.c)
            .collect();
        assert_eq!(text.chars().filter(|&c| c == 'x').count(), 100);
        assert!(text.contains('y'));
    }
    #[test]
    #[ignore = "requires TSHELL_SSH_TEST_HOST and key authentication"]
    fn real_plain_ssh_accepts_input() {
        let host = std::env::var("TSHELL_SSH_TEST_HOST").unwrap();
        let session = Session::ssh(std::env::current_dir().unwrap(), &host, None).unwrap();
        session.resize(24, 100);
        // The complete marker is absent from the input, so terminal echo cannot pass this test.
        session
            .input(b"printf '%s%s\\n' TSHELL_PLAIN_ SSH_OK\r".to_vec())
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let text: String = session
                .term
                .lock()
                .grid()
                .display_iter()
                .map(|c| c.c)
                .collect();
            if text.contains("TSHELL_PLAIN_SSH_OK") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "SSH did not produce marker: {text}"
            );
            thread::sleep(std::time::Duration::from_millis(25));
        }
        session.input(b"exit\r".to_vec()).unwrap();
    }

    #[test]
    fn ssh_destination_cannot_inject_options() {
        assert!(validate_destination("-oProxyCommand=bad").is_err());
        assert!(validate_destination("host\nanything").is_err());
        assert_eq!(validate_destination(" user@host ").unwrap(), "user@host");
    }

    #[test]
    fn real_pty_accepts_input_and_updates_terminal_grid() {
        let mut command = CommandBuilder::new(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" });
        if cfg!(windows) {
            command.args(["/Q", "/D"]);
        }
        let session = Session::spawn(command).unwrap();
        session.resize(24, 80);
        session.input(b"echo TSHELL_PTY_OK\r\n".to_vec()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            let text: String = session
                .term
                .lock()
                .grid()
                .display_iter()
                .map(|c| c.c)
                .collect();
            if text.contains("TSHELL_PTY_OK") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "PTY did not produce expected output: {text}"
            );
            thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(session.bytes_read.load(Ordering::Relaxed) > 0);
    }
}
