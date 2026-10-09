use super::*;
use anyhow::Context;
use russh::server;

struct Server;
impl server::Handler for Server {
    type Error = anyhow::Error;
    async fn auth_none(&mut self, _: &str) -> Result<server::Auth> {
        Ok(server::Auth::Accept)
    }
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<server::Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<()> {
        let mut socket = tokio::net::TcpStream::connect((host, port as u16)).await?;
        reply.accept().await;
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
        });
        Ok(())
    }
    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut server::Session,
    ) -> Result<bool> {
        let listener = tokio::net::TcpListener::bind((address, *port as u16)).await?;
        let handle = session.handle();
        let address = address.to_owned();
        let port = *port;
        tokio::spawn(async move {
            let (mut socket, peer) = listener.accept().await.unwrap();
            let channel = handle
                .channel_open_forwarded_tcpip(
                    address,
                    port,
                    peer.ip().to_string(),
                    u32::from(peer.port()),
                )
                .await
                .unwrap();
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
        });
        Ok(true)
    }
}

#[test]
fn local_remote_and_socks_forward_payloads_count_bytes_and_stop() -> Result<()> {
    runtime().block_on(async {
        for kind in [
            ForwardKind::Local,
            ForwardKind::Remote,
            ForwardKind::Dynamic,
        ] {
            let key = russh::keys::PrivateKey::from_openssh(include_str!(
                "../../../../tests/fixtures/ssh-test-host-key"
            ))?;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let ssh_port = listener.local_addr()?.port();
            let trust = tempfile::tempdir()?;
            let known_hosts = trust.path().join("known_hosts");
            std::fs::write(
                &known_hosts,
                format!(
                    "[127.0.0.1]:{ssh_port} {}\n",
                    key.public_key().to_openssh()?
                ),
            )?;
            let config = Arc::new(server::Config {
                keys: vec![key],
                auth_rejection_time: Duration::ZERO,
                auth_rejection_time_initial: Some(Duration::ZERO),
                ..Default::default()
            });
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await?;
                server::run_stream(config, socket, Server).await?.await
            });
            let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let echo_port = echo.local_addr()?.port();
            let echo = tokio::spawn(async move {
                let (socket, _) = echo.accept().await?;
                let (mut reader, mut writer) = socket.into_split();
                tokio::io::copy(&mut reader, &mut writer).await?;
                writer.shutdown().await
            });
            let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let listen_port = reservation.local_addr()?.port();
            drop(reservation);
            let rule = LocalForward {
                kind,
                bind_host: "127.0.0.1".into(),
                local_port: listen_port,
                target_host: "127.0.0.1".into(),
                target_port: echo_port,
            };
            let host = SshConfig {
                destination: "127.0.0.1".into(),

                user: "fixture".into(),
                port: Some(ssh_port),
                identity_file: None,
            };
            let stats = Statistics::new();
            let worker_stats = stats.clone();
            let (cancel, cancelled) = tokio::sync::watch::channel(false);
            let (updates, events) = async_channel::bounded(1);
            let worker = tokio::spawn(async move {
                let port = rule.local_port;
                serve_with(
                    &rule,
                    worker_stats,
                    updates,
                    cancelled,
                    move |sender| async move {
                        connection::connect_fixture(
                            &host,
                            &known_hosts,
                            (kind == ForwardKind::Remote).then_some((port, sender)),
                        )
                        .await
                    },
                )
                .await
            });
            tokio::time::timeout(Duration::from_secs(5), async {
                while stats.phase() != crate::forwarding::Phase::Running {
                    events.recv().await?;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await??;
            let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", listen_port))
                .await
                .with_context(|| format!("connect {kind:?} listener"))?;
            if kind == ForwardKind::Dynamic {
                socket.write_all(&[5, 1, 0]).await?;
                let mut greeting = [0; 2];
                socket.read_exact(&mut greeting).await?;
                assert_eq!(greeting, [5, 0]);
                let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
                request.extend(echo_port.to_be_bytes());
                socket.write_all(&request).await?;
                let mut response = [0; 10];
                socket.read_exact(&mut response).await?;
                assert_eq!(response[1], 0);
            }
            socket.write_all(b"hello").await?;
            socket.shutdown().await?;
            let mut reply = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut reply))
                .await?
                .with_context(|| format!("read {kind:?} forwarded payload"))?;
            assert_eq!(reply, b"hello");
            assert_eq!(stats.counters(), (5, 5));
            cancel.send(true)?;
            tokio::time::timeout(Duration::from_secs(2), worker).await???;
            tokio::time::timeout(Duration::from_secs(2), echo).await???;
            let shutdown = tokio::time::timeout(Duration::from_secs(2), server).await??;
            if let Err(error) = shutdown {
                // Cancellation can close TCP before the SSH disconnect is flushed on Windows.
                let cancelled_transport = error.chain().any(|cause| {
                    let io = cause
                        .downcast_ref::<std::io::Error>()
                        .or_else(|| match cause.downcast_ref::<russh::Error>() {
                            Some(russh::Error::IO(error)) => Some(error),
                            _ => None,
                        });
                    io.is_some_and(|error| {
                        matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionAborted
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::UnexpectedEof
                                | std::io::ErrorKind::BrokenPipe
                        )
                    })
                });
                ensure!(cancelled_transport, "{kind:?} fixture shutdown: {error}");
            }
            assert!(
                tokio::net::TcpStream::connect(("127.0.0.1", listen_port))
                    .await
                    .is_err()
            );
        }
        Ok(())
    })
}
