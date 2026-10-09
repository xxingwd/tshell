//! Native TCP forwarding reuses the authentication and host-key verification boundary.
use super::*;
use crate::forwarding::{ForwardKind, LocalForward, Statistics};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(test)]
#[path = "forwarding_tests.rs"]
mod tests;

async fn socks_target(stream: &mut tokio::net::TcpStream) -> Result<(String, u16)> {
    let version = stream.read_u8().await?;
    let count = stream.read_u8().await?;
    ensure!(version == 5 && count > 0, "Invalid SOCKS5 greeting");
    let mut methods = vec![0; count as usize];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        stream.write_all(&[5, 255]).await?;
        bail!("SOCKS5 requires unauthenticated local access");
    }
    stream.write_all(&[5, 0]).await?;
    let mut request = [0; 4];
    stream.read_exact(&mut request).await?;
    ensure!(
        request[0] == 5 && request[1] == 1 && request[2] == 0,
        "Only SOCKS5 CONNECT is supported"
    );
    let host = match request[3] {
        1 => {
            let mut bytes = [0; 4];
            stream.read_exact(&mut bytes).await?;
            std::net::Ipv4Addr::from(bytes).to_string()
        }
        3 => {
            let count = stream.read_u8().await?;
            ensure!(count > 0, "Empty SOCKS5 hostname");
            let mut bytes = vec![0; count as usize];
            stream.read_exact(&mut bytes).await?;
            String::from_utf8(bytes)?
        }
        4 => {
            let mut bytes = [0; 16];
            stream.read_exact(&mut bytes).await?;
            std::net::Ipv6Addr::from(bytes).to_string()
        }
        _ => bail!("Unsupported SOCKS5 address"),
    };
    let port = stream.read_u16().await?;
    ensure!(
        port > 0 && !host.chars().any(char::is_control),
        "Invalid SOCKS5 destination"
    );
    Ok((host, port))
}

async fn copy_counted<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
    mut reader: R,
    mut writer: W,
    counter: Arc<std::sync::atomic::AtomicU64>,
) -> std::io::Result<()> {
    let mut bytes = [0; 32768];
    loop {
        let count = reader.read(&mut bytes).await?;
        if count == 0 {
            writer.shutdown().await?;
            return Ok(());
        }
        writer.write_all(&bytes[..count]).await?;
        counter.fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed);
    }
}

async fn relay(
    socket: tokio::net::TcpStream,
    channel: Channel<client::Msg>,
    stats: Statistics,
    remote: bool,
) -> Result<()> {
    let (socket_read, socket_write) = socket.into_split();
    let (ssh_read, ssh_write) = tokio::io::split(channel.into_stream());
    let (up, down) = if remote {
        (stats.received, stats.sent)
    } else {
        (stats.sent, stats.received)
    };
    tokio::try_join!(
        copy_counted(socket_read, ssh_write, up),
        copy_counted(ssh_read, socket_write, down)
    )?;
    Ok(())
}

pub async fn serve(
    host: &SshConfig,
    rule: &LocalForward,
    stats: Statistics,
    updates: async_channel::Sender<()>,
    cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    serve_with(rule, stats, updates, cancelled, |sender| {
        connection::connect_forward(
            host,
            (rule.kind == ForwardKind::Remote).then_some((rule.local_port, sender)),
        )
    })
    .await
}

async fn serve_with<F, Fut>(
    rule: &LocalForward,
    stats: Statistics,
    updates: async_channel::Sender<()>,
    mut cancelled: tokio::sync::watch::Receiver<bool>,
    connect: F,
) -> Result<()>
where
    F: FnOnce(tokio::sync::mpsc::Sender<Channel<client::Msg>>) -> Fut,
    Fut: std::future::Future<Output = Result<connection::Connection>>,
{
    let (forward_sender, mut forwarded) = tokio::sync::mpsc::channel(32);
    let connecting = connect(forward_sender);
    let transport = tokio::select! {
        _ = cancelled.changed() => return Ok(()),
        result = connecting => Arc::new(result?),
    };
    let setup = async {
        let listener = if rule.kind == ForwardKind::Remote {
            tokio::time::timeout(
                Duration::from_secs(10),
                transport
                    .handle
                    .tcpip_forward(&rule.bind_host, u32::from(rule.local_port)),
            )
            .await??;
            None
        } else {
            Some(tokio::net::TcpListener::bind((rule.bind_host.as_str(), rule.local_port)).await?)
        };
        Ok::<_, anyhow::Error>(listener)
    };
    let configured =
        tokio::select! { _ = cancelled.changed() => Ok(None), result = setup => result };
    let listener = match configured {
        Ok(listener) if !*cancelled.borrow() => listener,
        result => {
            let _ = transport
                .handle
                .disconnect(
                    russh::Disconnect::ByApplication,
                    "forward setup stopped",
                    "en",
                )
                .await;
            return result.map(|_| ());
        }
    };
    stats.set_phase(crate::forwarding::Phase::Running);
    let _ = updates.try_send(());
    let mut relays = tokio::task::JoinSet::new();
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let mut reported = (0, 0);
    let result = async {
        loop {
            tokio::select! {
                biased;
                _ = cancelled.changed() => return Ok(()),
                _ = interval.tick() => {
                    if transport.handle.is_closed() { return Err(crate::diagnostic!("ssh.forward_transport_closed").into()); }
                    let counters = stats.counters();
                    if counters != reported { reported = counters; let _ = updates.try_send(()); }
                }
                Some(_) = relays.join_next(), if !relays.is_empty() => {},
                channel = forwarded.recv(), if rule.kind == ForwardKind::Remote => {
                    let Some(channel) = channel else { bail!("Remote forwarding receiver closed"); };
                    if relays.len() >= 32 { let _ = channel.close().await; continue; }
                    let rule = rule.clone(); let stats = stats.clone();
                    relays.spawn(async move {
                        let socket = tokio::time::timeout(Duration::from_secs(10), tokio::net::TcpStream::connect((rule.target_host.as_str(), rule.target_port))).await??;
                        relay(socket, channel, stats, true).await
                    });
                }
                accepted = async { match &listener { Some(listener) => listener.accept().await, None => std::future::pending().await } }, if listener.is_some() => {
                    let (mut socket, peer) = accepted?;
                    if relays.len() >= 32 { continue; }
                    let transport = transport.clone(); let stats = stats.clone(); let rule = rule.clone();
                    relays.spawn(async move {
                        let destination = if rule.kind == ForwardKind::Dynamic {
                            tokio::time::timeout(Duration::from_secs(5), socks_target(&mut socket)).await??
                        } else { (rule.target_host.clone(), rule.target_port) };
                        let opened = tokio::time::timeout(Duration::from_secs(10), transport.handle.channel_open_direct_tcpip(destination.0, u32::from(destination.1), peer.ip().to_string(), u32::from(peer.port()))).await;
                        let channel = match opened {
                            Ok(Ok(channel)) => channel,
                            _ => { if rule.kind == ForwardKind::Dynamic { let _ = socket.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await; } return Ok(()); }
                        };
                        if rule.kind == ForwardKind::Dynamic { socket.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?; }
                        relay(socket, channel, stats, false).await
                    });
                }
            }
        }
    }.await;
    relays.abort_all();
    while relays.join_next().await.is_some() {}
    let _ = transport
        .handle
        .disconnect(russh::Disconnect::ByApplication, "forward stopped", "en")
        .await;
    result
}
