//! Fixed-session control clients: each owns a dedicated SSH transport for commands and output.
use super::*;

const SUBSCRIPTION_COMMANDS: [&[u8]; 2] = [
    b"refresh-client -B 'tshell-titles:%*:#{q:pane_title}'\n",
    b"refresh-client -B 'tshell-paths:%*:#{q:pane_current_path}'\n",
];

pub(super) struct Streams {
    clients: BTreeMap<String, Stream>,
    opening: BTreeSet<String>,
    retry: BTreeMap<String, (Instant, String)>,
}

pub(super) struct Stream {
    _control: crate::ssh_pool::ChannelGuard,
    stdin: crate::ssh_pool::Writer,
    pending: VecDeque<Response>,
    ready: Option<std::sync::mpsc::SyncSender<()>>,
}

impl Streams {
    pub fn new() -> Self {
        Self {
            clients: BTreeMap::new(),
            opening: BTreeSet::new(),
            retry: BTreeMap::new(),
        }
    }

    pub fn reconcile(
        &mut self,
        config: &HostConfig,
        sessions: &[SessionInfo],
        tx: &SyncSender<Message>,
    ) -> Option<Instant> {
        self.clients
            .retain(|id, _| sessions.iter().any(|s| &s.id == id));
        self.opening
            .retain(|id| sessions.iter().any(|s| &s.id == id));
        self.retry
            .retain(|id, _| sessions.iter().any(|s| &s.id == id));
        for session in sessions {
            if self.clients.contains_key(&session.id) || self.opening.contains(&session.id) {
                continue;
            }
            if let Some((time, _)) = self.retry.get(&session.id)
                && time.elapsed() < Duration::from_secs(10)
            {
                continue;
            }
            let id = session.id.clone();
            let host = config.clone();
            let updates = tx.clone();
            self.opening.insert(id.clone());
            thread::spawn(move || {
                let result =
                    open(&host, &id, updates.clone()).map_err(|error| format!("{error:#}"));
                let _ = updates.send(Message::StreamOpened(id, result));
            });
        }
        self.retry_deadline()
    }

    pub fn opened(&mut self, session: String, result: Result<Stream, String>) -> Result<bool> {
        if !self.opening.remove(&session) {
            return Ok(false);
        }
        match result {
            Ok(mut stream) => {
                let ready = stream.ready.take();
                self.retry.remove(&session);
                self.clients.insert(session, stream);
                if let Some(ready) = ready {
                    let _ = ready.send(());
                }
                Ok(true)
            }
            Err(reason) => {
                self.retry.insert(session, (Instant::now(), reason.clone()));
                Err(anyhow::anyhow!(reason))
            }
        }
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

    pub fn is_opening(&self, session: &str) -> bool {
        self.opening.contains(session)
    }

    pub fn is_retrying(&self, session: &str) -> bool {
        self.retry.contains_key(session)
    }

    pub fn retry_deadline(&self) -> Option<Instant> {
        self.retry
            .iter()
            .filter(|(id, _)| !self.clients.contains_key(*id) && !self.opening.contains(*id))
            .map(|(_, (started, _))| *started + Duration::from_secs(10))
            .min()
    }

    pub fn send(
        &mut self,
        session: &str,
        request: Request,
    ) -> Result<(), (anyhow::Error, Request)> {
        let Some(stream) = self.clients.get_mut(session) else {
            return Err((
                anyhow::anyhow!("{}", crate::t!("tmux.channel_closed")),
                request,
            ));
        };
        if let Err(error) = stream
            .stdin
            .write_all(format!("{}\n", request.command).as_bytes())
        {
            return Err((error.into(), request));
        }
        for _ in 0..request.leading_ignores {
            stream.pending.push_back(Response::Ignore);
        }
        stream.pending.push_back(request.response);
        Ok(())
    }
}

fn open(config: &HostConfig, session: &str, tx: SyncSender<Message>) -> Result<Stream> {
    let mut channel = crate::ssh_pool::BlockingChannel::exec_pty(
        config,
        format!(
            "exec {} -CC attach-session -t {}",
            config.tmux()?,
            q(session)
        ),
        100,
        28,
    )?;
    write_subscriptions(&mut channel.stdin)?;
    let session = session.to_owned();
    let (ready, start) = std::sync::mpsc::sync_channel(0);
    thread::spawn(move || {
        if start.recv().is_err() {
            return;
        }
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
        ready: Some(ready),
    })
}

fn write_subscriptions(stdin: &mut impl Write) -> io::Result<()> {
    for command in SUBSCRIPTION_COMMANDS {
        stdin.write_all(command)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_commands_use_tmux_format_braces() {
        let mut sent = Vec::new();
        write_subscriptions(&mut sent).unwrap();
        assert_eq!(
            sent,
            b"refresh-client -B 'tshell-titles:%*:#{q:pane_title}'\nrefresh-client -B 'tshell-paths:%*:#{q:pane_current_path}'\n"
        );
    }

    #[test]
    fn retry_deadline_survives_discovery_while_stream_is_closed() {
        let mut streams = Streams::new();
        let failed_at = Instant::now();
        streams
            .retry
            .insert("$1".into(), (failed_at, "closed".into()));
        let host = HostConfig {
            destination: "unused".into(),
            name: String::new(),
            user: String::new(),
            port: None,
            identity_file: None,
            tmux: true,
            socket: None,
        };
        let (tx, _rx) = mpsc::sync_channel(1);
        let sessions = [SessionInfo {
            id: "$1".into(),
            ..Default::default()
        }];
        assert_eq!(
            streams.reconcile(&host, &sessions, &tx),
            Some(failed_at + Duration::from_secs(10))
        );
        assert!(streams.is_retrying("$1"));
        streams.opening.insert("$1".into());
        assert_eq!(streams.reconcile(&host, &sessions, &tx), None);
    }
}
