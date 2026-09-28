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
        let size = buf.len().min(32768);
        self.sender
            .send_blocking(buf[..size].to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(size)
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
    pub fn exec(host: &HostConfig, command: String) -> Result<Self> {
        let host = host.clone();
        let (ready, result) = std::sync::mpsc::sync_channel(1);
        let (cancel, cancelled) = async_channel::bounded(1);
        let (input, inputs) = async_channel::bounded::<Vec<u8>>(64);
        let (output, outputs) = async_channel::bounded(64);
        let (errors, error_rx) = async_channel::bounded(16);
        runtime().spawn(async move {
            let mut channel = match super::exec_dedicated(&host, &command).await { Ok(channel) => { let _ = ready.send(Ok(())); channel }, Err(error) => { let _ = ready.send(Err(error)); return; } };
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
