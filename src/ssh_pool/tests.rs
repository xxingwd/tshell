use super::*;
use tokio::time::timeout;

#[test]
#[ignore = "requires TSHELL_SSH_TEST_HOST; verifies dedicated session transports and auxiliary isolation"]
fn dedicated_sessions_and_auxiliary_transport_are_isolated() -> Result<()> {
    runtime().block_on(async {
        let host = host();
        let command = "printf '%s' \"$SSH_CONNECTION\"";
        let mut first = exec_dedicated(&host, command).await?;
        let mut second = exec_dedicated(&host, command).await?;
        let mut auxiliary = exec_aux(&host, command).await?;
        let a = collect(&mut first).await?;
        let b = collect(&mut second).await?;
        let c = collect(&mut auxiliary).await?;
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        let mut again = exec_aux(&host, command).await?;
        assert_eq!(collect(&mut again).await?, c);
        drop(first);
        let mut still_alive = exec_aux(&host, "printf still-alive").await?;
        assert_eq!(collect(&mut still_alive).await?, "still-alive");
        Ok(())
    })
}

#[test]
#[ignore = "requires TSHELL_SSH_TEST_HOST; auxiliary requests queue and reuse one transport"]
fn auxiliary_capacity_queues_until_a_channel_is_released() -> Result<()> {
    runtime().block_on(async {
        let host = host();
        let mut channels = Vec::new();
        for _ in 0..8 {
            channels.push(exec_aux(&host, "cat").await?);
        }
        let request_host = host.clone();
        let mut waiting =
            runtime().spawn(async move { exec_aux(&request_host, "printf ready").await });
        assert!(
            timeout(Duration::from_millis(100), &mut waiting)
                .await
                .is_err()
        );
        let released = channels.pop().unwrap();
        released.close().await?;
        drop(released);
        let mut next = timeout(Duration::from_secs(5), waiting).await???;
        assert_eq!(collect(&mut next).await?, "ready");
        assert_eq!(channels[0]._entry.connection.lock().await.len(), 1);
        for channel in channels {
            channel.close().await?;
        }
        Ok(())
    })
}

#[test]
#[ignore = "requires TSHELL_SSH_TEST_HOST; holds 12 shells while opening short-lived channels"]
fn channel_capacity_preserves_shells_and_tools() -> Result<()> {
    runtime().block_on(async {
        let host = host();
        let mut shells = Vec::new();
        for _ in 0..12 {
            shells.push(exec(&host, "cat").await?);
        }
        let transports = shells[0]._entry.connection.lock().await.len();
        for _ in 0..16 {
            let mut query = exec(&host, "printf tool-ok").await?;
            assert_eq!(collect(&mut query).await?, "tool-ok");
            let mut files = open(&host).await?;
            files.request_subsystem(true, "sftp").await?;
            confirmed(&mut files).await?;
            let sftp = russh_sftp::client::SftpSession::new(files.into_stream()).await?;
            assert!(!sftp.canonicalize(".").await?.is_empty());
            sftp.close().await?;
        }
        assert!(
            shells[0]._entry.connection.lock().await.len() <= transports + 1,
            "short-lived tools kept opening new transports instead of reusing capacity"
        );
        for shell in &mut shells {
            shell.data(&b"still-alive\n"[..]).await?;
            shell.eof().await?;
            assert_eq!(collect(shell).await?, "still-alive\n");
        }
        Ok(())
    })
}

fn host() -> HostConfig {
    HostConfig {
        destination: std::env::var("TSHELL_SSH_TEST_HOST").unwrap(),
        name: String::new(),
        user: String::new(),
        port: None,
        identity_file: None,
        tmux: false,

        socket: None,
    }
}
async fn collect(channel: &mut SharedChannel) -> Result<String> {
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(10), async {
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } => bytes.extend_from_slice(&data),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
    })
    .await?;
    Ok(String::from_utf8(bytes)?)
}
#[test]
#[ignore = "requires TSHELL_SSH_TEST_HOST; verifies channel isolation and same remote TCP tuple"]
fn shared_channels_and_reconnect() -> Result<()> {
    runtime().block_on(async {
        let host = host();
        let (first, second) = tokio::join!(
            exec(&host, "printf '%s' \"$SSH_CONNECTION\""),
            exec(&host, "printf '%s' \"$SSH_CONNECTION\"")
        );
        let (mut first, mut second) = (first?, second?);
        assert!(Arc::ptr_eq(&first._entry, &second._entry));
        let tuple = collect(&mut first).await?;
        assert_eq!(tuple.split_whitespace().count(), 4);
        assert_eq!(tuple, collect(&mut second).await?);
        let mut shell = exec(&host, "cat").await?;
        let mut files = open(&host).await?;
        files.request_subsystem(true, "sftp").await?;
        confirmed(&mut files).await?;
        let sftp = russh_sftp::client::SftpSession::new(files.into_stream()).await?;
        assert!(!sftp.read("/etc/hostname").await?.is_empty());
        sftp.close().await?;
        drop(sftp);
        drop(first);
        drop(second);
        shell.data(&b"still-alive\n"[..]).await?;
        shell.eof().await?;
        assert_eq!(collect(&mut shell).await?, "still-alive\n");
        // Disconnect only this test's pooled transport. New channels must authenticate anew.
        {
            let connection = shell._entry.connection.lock().await;
            connection
                .first()
                .unwrap()
                .handle
                .disconnect(russh::Disconnect::ByApplication, "reconnect test", "en")
                .await?;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !shell
            ._entry
            .connection
            .lock()
            .await
            .first()
            .unwrap()
            .handle
            .is_closed()
        {
            ensure!(
                std::time::Instant::now() < deadline,
                "transport did not close"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut restored = exec(&host, "printf '%s' \"$SSH_CONNECTION\"").await?;
        let restored_tuple = collect(&mut restored).await?;
        assert_ne!(tuple, restored_tuple);
        Ok(())
    })
}
