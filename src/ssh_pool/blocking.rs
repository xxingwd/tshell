//! Bounded adapters for the existing blocking tmux control parser.
use super::*;
use std::io::{self, Read, Write};

pub(crate) struct ChannelGuard {
    cancel: async_channel::Sender<()>,
}
impl ChannelGuard {
    pub fn close(&self) {
        let _ = self.cancel.try_send(());
    }
}
impl Drop for ChannelGuard {
    fn drop(&mut self) {
        self.close();
    }
}
pub(crate) struct Reader {
    receiver: async_channel::Receiver<Vec<u8>>,
    pending: io::Cursor<Vec<u8>>,
}
impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.pending.position() as usize >= self.pending.get_ref().len() {
            match self.receiver.recv_blocking() {
                Ok(bytes) => self.pending = io::Cursor::new(bytes),
                Err(_) => return Ok(0),
            }
        }
        self.pending.read(buf)
    }
}
pub(crate) struct Writer {
    sender: async_channel::Sender<Vec<u8>>,
}
impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sender
            .try_send(buf.to_vec())
            .map_err(|error| match error {
                async_channel::TrySendError::Full(_) => io::Error::from(io::ErrorKind::WouldBlock),
                async_channel::TrySendError::Closed(_) => {
                    io::Error::from(io::ErrorKind::BrokenPipe)
                }
            })?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(crate) struct BlockingChannel {
    pub stdin: Writer,
    pub stdout: Reader,
    pub stderr: Reader,
    pub control: ChannelGuard,
}
impl BlockingChannel {
    pub fn exec_pty(host: &HostConfig, command: String, cols: u32, rows: u32) -> Result<Self> {
        let host = host.clone();
        let (ready, result) = std::sync::mpsc::sync_channel(1);
        let (cancel, cancelled) = async_channel::bounded(1);
        let (input, inputs) = async_channel::bounded::<Vec<u8>>(64);
        let (output, outputs) = async_channel::bounded(64);
        let (errors, error_rx) = async_channel::bounded(16);
        runtime().spawn(async move {
            let result = super::exec_dedicated_pty(&host, &command, cols, rows).await;
            let mut channel = match result { Ok(channel) => { let _ = ready.send(Ok(())); channel }, Err(error) => { let _ = ready.send(Err(error)); return; } };
            let mut input_open = true;
            loop {
                tokio::select! {
                    _ = cancelled.recv() => break,
                    data = inputs.recv(), if input_open => {
                        match data { Ok(data) => { if tokio::select! { _ = cancelled.recv() => break, result = channel.data(data.as_slice()) => result }.is_err() { break; } }, Err(_) => { input_open = false; let _ = channel.eof().await; } }
                    }
                    message = channel.wait() => {
                        let sent = match message {
                            Some(ChannelMsg::Data { data }) => tokio::select! { _ = cancelled.recv() => break, sent = output.send(data.to_vec()) => sent },
                            Some(ChannelMsg::ExtendedData { data, .. }) => tokio::select! { _ = cancelled.recv() => break, sent = errors.send(data.to_vec()) => sent },
                            Some(ChannelMsg::Close) | None => break,
                            _ => continue,
                        };
                        if sent.is_err() { break; }
                    }
                }
            }
            let _ = channel.close().await;
        });
        result.recv().context(crate::t!("ssh.worker_stopped"))??;
        Ok(Self {
            stdin: Writer { sender: input },
            stdout: Reader {
                receiver: outputs,
                pending: io::Cursor::new(Vec::new()),
            },
            stderr: Reader {
                receiver: error_rx,
                pending: io::Cursor::new(Vec::new()),
            },
            control: ChannelGuard { cancel },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_control_write_queue_returns_without_blocking() {
        let (sender, _receiver) = async_channel::bounded(1);
        let mut writer = Writer { sender };
        writer.write_all(b"first\n").unwrap();
        assert_eq!(
            writer.write_all(b"second\n").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn long_control_write_is_enqueued_as_one_message() {
        let (sender, receiver) = async_channel::bounded(1);
        let mut writer = Writer { sender };
        let command = vec![b'x'; 32769];
        writer.write_all(&command).unwrap();
        assert_eq!(receiver.try_recv().unwrap(), command);
    }
}
