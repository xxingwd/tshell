//! Fixed-session control clients: each owns a dedicated SSH transport for commands and output.
use super::*;

pub(super) struct Streams {
    clients: BTreeMap<String, Stream>,
    retry: BTreeMap<String, (Instant, String)>,
}

struct Stream {
    _control: crate::ssh_pool::ChannelGuard,
    stdin: crate::ssh_pool::Writer,
    pending: VecDeque<Response>,
}

impl Streams {
    pub fn new() -> Self {
        Self {
            clients: BTreeMap::new(),
            retry: BTreeMap::new(),
        }
    }

    pub fn reconcile(
        &mut self,
        config: &HostConfig,
        sessions: &[SessionInfo],
        tx: &SyncSender<Message>,
    ) -> Result<()> {
        self.clients
            .retain(|id, _| sessions.iter().any(|s| &s.id == id));
        self.retry
            .retain(|id, _| sessions.iter().any(|s| &s.id == id));
        let mut error = None;
        for session in sessions {
            if self.clients.contains_key(&session.id) {
                continue;
            }
            if let Some((time, reason)) = self.retry.get(&session.id)
                && time.elapsed() < Duration::from_secs(10)
            {
                error = Some(reason.clone());
                continue;
            }
            match open(config, &session.id, tx.clone()) {
                Ok(stream) => {
                    self.retry.remove(&session.id);
                    self.clients.insert(session.id.clone(), stream);
                }
                Err(reason) => {
                    let reason = format!("{reason:#}");
                    self.retry
                        .insert(session.id.clone(), (Instant::now(), reason.clone()));
                    error = Some(reason);
                }
            }
        }
        if let Some(error) = error {
            bail!("{error}");
        }
        Ok(())
    }

    pub fn remove(&mut self, session: &str, reason: String) -> Vec<Response> {
        let pending = self
            .clients
            .remove(session)
            .map(|stream| stream.pending.into_iter().collect())
            .unwrap_or_default();
        self.retry
            .insert(session.to_owned(), (Instant::now(), reason));
        pending
    }

    pub fn response(&mut self, session: &str) -> Option<Response> {
        self.clients.get_mut(session)?.pending.pop_front()
    }

    pub fn first(&self) -> Option<String> {
        self.clients.keys().next().cloned()
    }
    pub fn can_send(&self, session: &str) -> bool {
        self.clients
            .get(session)
            .is_some_and(|s| s.pending.len() < 16)
    }

    pub fn contains(&self, session: &str) -> bool {
        self.clients.contains_key(session)
    }

    pub fn send(&mut self, session: &str, request: Request) -> Result<()> {
        let stream = self
            .clients
            .get_mut(session)
            .context(crate::t!("tmux.channel_closed"))?;
        writeln!(stream.stdin, "{}", request.command)?;
        for _ in 0..request.leading_ignores {
            stream.pending.push_back(Response::Ignore);
        }
        stream.pending.push_back(request.response);
        Ok(())
    }
}

fn open(config: &HostConfig, session: &str, tx: SyncSender<Message>) -> Result<Stream> {
    let mut channel = crate::ssh_pool::BlockingChannel::exec(
        config,
        format!(
            "exec {} -C attach-session -t {}",
            config.tmux()?,
            q(session)
        ),
    )?;
    writeln!(
        channel.stdin,
        "refresh-client -B 'tshell-titles:%*:#{{q:pane_title}}'"
    )?;
    writeln!(
        channel.stdin,
        "refresh-client -B 'tshell-paths:%*:#{{q:pane_current_path}}'"
    )?;
    let session = session.to_owned();
    thread::spawn(move || {
        let mut parser = ControlParser::default();
        let mut bytes = [0; 32768];
        let result = (|| -> Result<()> {
            loop {
                let n = channel.stdout.read(&mut bytes)?;
                if n == 0 {
                    break;
                }
                for event in parser.push(&bytes[..n])? {
                    match &event {
                        ControlEvent::Line(line) if line.starts_with("%exit") => {
                            tx.send(Message::StreamEvent(session.clone(), event))?;
                            return Ok(());
                        }
                        ControlEvent::Line(line) if line.starts_with("%session-changed ") => {
                            // Respect servers configured to move clients after session destruction:
                            // this reader must never silently start duplicating another session.
                            if line.split_whitespace().nth(1) != Some(session.as_str()) {
                                return Ok(());
                            }
                        }
                        _ => {}
                    }
                    tx.send(Message::StreamEvent(session.clone(), event))?;
                }
            }
            Ok(())
        })();
        let _ = tx.send(Message::StreamClosed(
            session,
            result.err().map(|e| e.to_string()),
        ));
    });
    // Extended data is not part of the control protocol; drain it so it cannot block stdout.
    thread::spawn(move || {
        let _ = std::io::copy(&mut channel.stderr, &mut std::io::sink());
    });
    Ok(Stream {
        _control: channel.control,
        stdin: channel.stdin,
        pending: VecDeque::from([Response::Ignore, Response::Ignore, Response::Ignore]),
    })
}
