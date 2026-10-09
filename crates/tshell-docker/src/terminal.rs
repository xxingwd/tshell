//! Exec IO is independent of the panel and retains the original Docker connection.
use anyhow::{Result, bail};
use bollard::{
    Docker,
    exec::{StartExecOptions, StartExecResults},
    models::ExecConfig,
    query_parameters::ResizeExecOptionsBuilder,
};
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tshell_host::Failure;
enum Command {
    Input(Vec<u8>),
}
#[derive(Clone)]
pub struct Input(Arc<Control>);
struct Control {
    sender: async_channel::Sender<Command>,
    task: tokio::task::AbortHandle,
    size: Arc<parking_lot::Mutex<(u16, u16)>>,
    resized: Arc<tokio::sync::Notify>,
}
impl Drop for Control {
    fn drop(&mut self) {
        self.sender.close();
        self.task.abort();
    }
}
impl Input {
    pub fn send(&self, bytes: Vec<u8>) -> Result<()> {
        self.0
            .sender
            .try_send(Command::Input(bytes))
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
    pub fn resize(&self, rows: u16, cols: u16) {
        *self.0.size.lock() = (rows, cols);
        self.0.resized.notify_one();
    }
}
pub struct TerminalIo {
    pub input: Input,
    pub output: async_channel::Receiver<Result<Vec<u8>, Failure>>,
}
pub async fn connect(
    docker: Docker,
    container: String,
    command: Vec<String>,
    rows: u16,
    cols: u16,
) -> Result<TerminalIo> {
    let inspected = docker.inspect_container(&container, None).await?;
    anyhow::ensure!(
        inspected
            .state
            .as_ref()
            .is_some_and(|state| state.running == Some(true)),
        tshell_host::diagnostic!("docker.container_not_running")
    );
    let exec = docker
        .create_exec(
            &container,
            ExecConfig {
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(true),
                cmd: Some(command),
                ..Default::default()
            },
        )
        .await?;
    let StartExecResults::Attached {
        mut output,
        mut input,
    } = docker
        .start_exec(
            &exec.id,
            Some(StartExecOptions {
                tty: true,
                detach: false,
                output_capacity: Some(64 * 1024),
            }),
        )
        .await?
    else {
        bail!(tshell_host::diagnostic!("docker.exec_detached"));
    };
    docker
        .resize_exec(
            &exec.id,
            ResizeExecOptionsBuilder::default()
                .h(rows as i32)
                .w(cols as i32)
                .build(),
        )
        .await?;
    let (sender, commands) = async_channel::bounded(64);
    let (events, receiver) = async_channel::bounded(64);
    let size = Arc::new(parking_lot::Mutex::new((rows, cols)));
    let resized = Arc::new(tokio::sync::Notify::new());
    let worker_size = size.clone();
    let worker_resized = resized.clone();
    let task = tshell_host::ssh::runtime().spawn(async move {
        let result: Result<()> = async {
            let mut applied = (rows, cols);
            loop {
                tokio::select! {
                    command = commands.recv() => match command {
                        Ok(Command::Input(bytes)) => {
                            apply_size(&docker, &exec.id, &worker_size, &mut applied).await?;
                            input.write_all(&bytes).await?;
                        }
                        Err(_) => { input.shutdown().await?; break; }
                    },
                    _ = worker_resized.notified() => apply_size(&docker, &exec.id, &worker_size, &mut applied).await?,
                    chunk = output.next() => match chunk {
                        Some(Ok(bytes)) => if events.send(Ok(bytes.into_bytes().to_vec())).await.is_err() { break; },
                        Some(Err(error)) => return Err(error.into()),
                        None => break,
                    }
                }
            }
            Ok(())
        }.await;
        if let Err(error) = result { let _ = events.send(Err(Failure::from(&error))).await; }
    });
    let input = Input(Arc::new(Control {
        sender,
        task: task.abort_handle(),
        size,
        resized,
    }));
    Ok(TerminalIo {
        input,
        output: receiver,
    })
}

async fn apply_size(
    docker: &Docker,
    exec: &str,
    size: &parking_lot::Mutex<(u16, u16)>,
    applied: &mut (u16, u16),
) -> Result<()> {
    let requested = *size.lock();
    if requested != *applied {
        docker
            .resize_exec(
                exec,
                ResizeExecOptionsBuilder::default()
                    .h(requested.0 as i32)
                    .w(requested.1 as i32)
                    .build(),
            )
            .await?;
        *applied = requested;
    }
    Ok(())
}
