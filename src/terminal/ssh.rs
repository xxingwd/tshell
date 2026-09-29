use super::*;

impl Session {
    #[cfg(test)]
    pub fn ssh(cwd: PathBuf, destination: &str, port: Option<u16>) -> Result<Arc<Self>> {
        Self::ssh_in(cwd, destination, port, None)
    }

    #[cfg(test)]
    pub fn ssh_in(
        _cwd: PathBuf,
        destination: &str,
        port: Option<u16>,
        remote_directory: Option<&str>,
    ) -> Result<Arc<Self>> {
        validate_destination(destination)?;
        let host = crate::tmux_client::HostConfig {
            destination: destination.into(),
            user: String::new(),
            port,
            identity_file: None,
            name: String::new(),
            tmux: false,

            socket: None,
        };
        Self::ssh_config_in(host, remote_directory)
    }

    pub fn ssh_config_in(
        host: crate::tmux_client::HostConfig,
        remote_directory: Option<&str>,
    ) -> Result<Arc<Self>> {
        validate_destination(&host.destination)?;
        let directory = remote_directory.map(str::to_owned);
        let (sender, receiver) = async_channel::bounded(256);
        let input = sender.clone();
        let mut screen = Self::remote(
            crate::t!("term.shell").to_string(),
            28,
            100,
            Arc::new(move |data| {
                input
                    .try_send(Command::Input(data))
                    .context(crate::t!("ssh.input_closed"))
            }),
        );
        Arc::get_mut(&mut screen).unwrap().ssh_commands = Some(sender);
        screen.ready.store(false, Ordering::Release);
        let weak = Arc::downgrade(&screen);
        crate::ssh_pool::runtime().spawn(async move {
            let result = async {
                let mut channel = crate::ssh_pool::open(&host).await?;
                channel.request_pty(true, "xterm-256color", 100, 28, 0, 0, &[]).await?;
                crate::ssh_pool::confirmed(&mut channel).await?;
                if let Some(path) = directory {
                    let quoted = format!("'{}'", path.replace('\'', "'\\''"));
                    channel.exec(true, format!("cd -- {quoted} && exec \"${{SHELL:-/bin/sh}}\" -l")).await?;
                } else { channel.request_shell(true).await?; }
                crate::ssh_pool::confirmed(&mut channel).await?;
                let mut clipboard = crate::terminal_clipboard::Clipboard::default();
                let Some(screen) = weak.upgrade() else { return Ok(()); };
                screen.ready.store(true, Ordering::Release);
                screen.metadata.notify();
                drop(screen);
                loop {
                    tokio::select! {
                        command = receiver.recv() => match command {
                            Ok(Command::Input(data)) => channel.data(data.as_slice()).await?,
                            Ok(Command::Resize { rows, cols, generation }) => {
                                channel.window_change(cols as u32, rows as u32, 0, 0).await?;
                                if let Some(screen) = weak.upgrade() {
                                    screen.resize_barrier.complete(generation);
                                }
                            }
                            Ok(Command::Stop) | Err(_) => break,
                        },
                        message = channel.wait() => match message {
                            Some(russh::ChannelMsg::Data { data }) | Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                                let Some(screen) = weak.upgrade() else { break; };
                                clipboard.advance(&data);
                                screen.remote_output(&data);
                            }
                            Some(russh::ChannelMsg::Close) | None => break,
                            _ => {}
                        }
                    }
                }
                let _ = channel.close().await;
                Ok::<_, anyhow::Error>(())
            }.await;
            if let Some(screen) = weak.upgrade() {
                if let Err(error) = result {
                    screen.resize_barrier.fail();
                    *screen.error.lock() = Some(format!("{error:#}"));
                }
                screen.resize_barrier.fail();
                screen.exited.store(true, Ordering::Release);
                screen.metadata.notify();
                screen.updates.notify();
            }
        });
        Ok(screen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires TSHELL_SSH_TEST_HOST; checks actual remote PTY window-change"]
    fn shared_ssh_pty_resize() {
        let host = std::env::var("TSHELL_SSH_TEST_HOST").unwrap();
        let screen = Session::ssh(std::env::current_dir().unwrap(), &host, None).unwrap();
        screen.resize(41, 123);
        screen
            .input(b"printf 'size:'; stty size\r".to_vec())
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let text: String = screen
                .term
                .lock()
                .grid()
                .display_iter()
                .map(|cell| cell.c)
                .collect();
            if text.contains("size:41 123") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "PTY resize missing: {:?}; {text}",
                screen.error.lock()
            );
            thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
