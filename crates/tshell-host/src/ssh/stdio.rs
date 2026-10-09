//! A non-PTY byte stream with stderr kept outside the protocol stream.
use super::*;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
#[derive(Clone, Debug)]
struct StreamError(Arc<dyn std::error::Error + Send + Sync>);
impl std::fmt::Display for StreamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}
impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}
pub struct StdioStream {
    stream: DuplexStream,
    cancel: async_channel::Sender<()>,
    failure: Arc<parking_lot::Mutex<Option<StreamError>>>,
}
impl Drop for StdioStream {
    fn drop(&mut self) {
        let _ = self.cancel.try_send(());
    }
}
impl AsyncRead for StdioStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, buf);
        if matches!(result, Poll::Ready(Ok(()))) && before == buf.filled().len() {
            if let Some(error) = self.failure.lock().as_ref() {
                return Poll::Ready(Err(io::Error::other(error.clone())));
            }
        }
        result
    }
}
impl AsyncWrite for StdioStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
impl SharedChannel {
    pub fn into_stdio(mut self) -> StdioStream {
        let (stream, peer) = tokio::io::duplex(64 * 1024);
        let (cancel, shutdown) = async_channel::bounded(1);
        let failure = Arc::new(parking_lot::Mutex::new(None));
        let error = failure.clone();
        let channel = self.channel.take().expect("owned SSH channel");
        let (mut receive, send) = channel.split();
        let lifetime = (self._entry.clone(), self._slot.take());
        runtime().spawn(async move {
            let _lifetime = lifetime;
            let (mut input, mut output) = tokio::io::split(peer);
            let mut bytes = [0; 16 * 1024];
            let mut input_open = true;
            let mut stderr = Vec::new();
            let mut status = None;
            let result: Result<()> = async {
                loop {
                    tokio::select! {
                        _ = shutdown.recv() => break,
                        read = input.read(&mut bytes), if input_open => {
                            let count = read?;
                            if count == 0 {
                                input_open = false;
                                send.eof().await?;
                            } else {
                                send.data(&bytes[..count]).await?;
                            }
                        },
                        message = receive.wait() => match message {
                            Some(ChannelMsg::Data { data }) => output.write_all(&data).await?,
                            Some(ChannelMsg::ExtendedData { data, .. }) => {
                                let remaining = (16usize * 1024).saturating_sub(stderr.len());
                                stderr.extend_from_slice(&data[..data.len().min(remaining)]);
                            }
                            Some(ChannelMsg::ExitStatus { exit_status }) => status = Some(exit_status),
                            Some(ChannelMsg::Close) | None => break,
                            _ => {}
                        }
                    }
                }
                Ok(())
            }.await;
            let failure = match result {
                Err(cause) => Some(cause),
                Ok(()) if status.is_some_and(|code| code != 0) => Some(crate::diagnostic!(
                    "ssh.command_failed", output = format!("{} ({})",String::from_utf8_lossy(&stderr).trim(),status.unwrap_or_default())
                ).into()),
                Ok(()) => None,
            };
            if let Some(cause) = failure {
                *error.lock() = Some(StreamError(Arc::from(cause.into_boxed_dyn_error())));
            }
            let _ = send.close().await;
        });
        StdioStream {
            stream,
            cancel,
            failure,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::{ChannelId, server};
    struct Server {
        closed: Arc<tokio::sync::Notify>,
    }
    impl server::Handler for Server {
        type Error = anyhow::Error;
        async fn auth_none(&mut self, _: &str) -> Result<server::Auth> {
            Ok(server::Auth::Accept)
        }
        async fn channel_open_session(
            &mut self,
            _: Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _: &mut server::Session,
        ) -> Result<()> {
            reply.accept().await;
            Ok(())
        }
        async fn exec_request(
            &mut self,
            channel: ChannelId,
            command: &[u8],
            session: &mut server::Session,
        ) -> Result<()> {
            session.channel_success(channel)?;
            session.extended_data(channel, 1, b"diagnostic on stderr".to_vec())?;
            session.data(channel, b"HTTP bytes".to_vec())?;
            if command == b"fail" {
                session.exit_status_request(channel, 7)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            Ok(())
        }
        async fn channel_close(&mut self, _: ChannelId, _: &mut server::Session) -> Result<()> {
            self.closed.notify_one();
            Ok(())
        }
    }
    #[test]
    fn stderr_never_enters_stdout_and_dropping_stream_closes_channel() -> Result<()> {
        runtime().block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                let key = russh::keys::PrivateKey::from_openssh(include_str!(
                    "../../../../tests/fixtures/ssh-test-host-key"
                ))?;
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
                let port = listener.local_addr()?.port();
                let trust = tempfile::tempdir()?;
                let known = trust.path().join("known_hosts");
                std::fs::write(
                    &known,
                    format!("[127.0.0.1]:{port} {}\n", key.public_key().to_openssh()?),
                )?;
                let config = Arc::new(server::Config {
                    keys: vec![key],
                    auth_rejection_time: Duration::ZERO,
                    auth_rejection_time_initial: Some(Duration::ZERO),
                    ..Default::default()
                });
                let closed = Arc::new(tokio::sync::Notify::new());
                let notification = closed.clone();
                let server = tokio::spawn(async move {
                    let (socket, _) = listener.accept().await?;
                    server::run_stream(
                        config,
                        socket,
                        Server {
                            closed: notification,
                        },
                    )
                    .await?
                    .await
                });
                let host = SshConfig {
                    destination: "127.0.0.1".into(),
                    user: "fixture".into(),
                    port: Some(port),
                    identity_file: None,
                };
                let connection = connection::connect_fixture(&host, &known, None).await?;
                let entry = Arc::new(Entry {
                    connection: tokio::sync::Mutex::new(vec![connection]),
                    connecting: tokio::sync::Mutex::new(()),
                    slots: Arc::new(tokio::sync::Semaphore::new(2)),
                    short_slots: Arc::new(tokio::sync::Semaphore::new(2)),
                });
                for command in ["fail", "stay"] {
                    let channel = entry.connection.lock().await[0]
                        .handle
                        .channel_open_session()
                        .await?;
                    let mut channel = SharedChannel {
                        channel: Some(channel),
                        _entry: entry.clone(),
                        _slot: Some(entry.slots.clone().acquire_owned().await?),
                    };
                    channel.exec(true, command).await?;
                    confirmed(&mut channel).await?;
                    let mut stream = channel.into_stdio();
                    let mut bytes = [0; 10];
                    stream.read_exact(&mut bytes).await?;
                    assert_eq!(&bytes, b"HTTP bytes");
                    if command == "fail" {
                        let error = stream.read_to_end(&mut Vec::new()).await.unwrap_err();
                        assert!(error.to_string().contains("diagnostic on stderr"));
                    }
                    drop(stream);
                    if command == "stay" {
                        closed.notified().await;
                    }
                }
                drop(entry);
                server.abort();
                Ok::<_, anyhow::Error>(())
            })
            .await?
        })
    }
}
