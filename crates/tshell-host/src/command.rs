use crate::{HostPath, SshConfig, diagnostic::ResultExt};
use anyhow::{Result, ensure};
use std::{collections::BTreeMap, process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Host {
    Local,
    Ssh(SshConfig),
}
impl Host {
    pub fn from_ssh(config: Option<&SshConfig>) -> Self {
        config.cloned().map(Self::Ssh).unwrap_or(Self::Local)
    }
}
pub struct CommandRequest {
    pub program: String,
    pub arguments: Vec<String>,
    pub directory: Option<HostPath>,
    pub environment: BTreeMap<String, String>,
    pub stdin: Vec<u8>,
    pub allowed_status: Vec<i32>,
    pub output_limit: usize,
    pub timeout: Duration,
}
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
impl CommandRequest {
    pub fn new(
        program: impl Into<String>,
        arguments: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            arguments: arguments.into_iter().map(Into::into).collect(),
            directory: None,
            environment: BTreeMap::new(),
            stdin: Vec::new(),
            allowed_status: vec![0],
            output_limit: 1024 * 1024,
            timeout: Duration::from_secs(20),
        }
    }
    pub fn remote_command(&self) -> Result<String> {
        ensure!(
            !self.program.contains('\0')
                && self
                    .arguments
                    .iter()
                    .all(|argument| !argument.contains('\0')),
            crate::diagnostic!("host.invalid_command")
        );
        let mut command = String::new();
        if let Some(directory) = &self.directory {
            let HostPath::Remote(directory) = directory else {
                anyhow::bail!(crate::diagnostic!("host.path_mismatch"));
            };
            command.push_str(&format!("cd {} && ", quote(directory.as_str())));
        }
        command.push_str("env ");
        for (key, value) in &self.environment {
            ensure!(
                !key.contains(['=', '\0']) && !value.contains('\0'),
                crate::diagnostic!("host.invalid_command")
            );
            command.push_str(&quote(&format!("{key}={value}")));
            command.push(' ');
        }
        command.push_str(&quote(&self.program));
        for argument in &self.arguments {
            command.push(' ');
            command.push_str(&quote(argument));
        }
        Ok(command)
    }
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
async fn limited(mut stream: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    (&mut stream)
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(
        bytes.len() <= limit,
        crate::diagnostic!("host.output_too_large")
    );
    Ok(bytes)
}
fn append_tail(bytes: &mut Vec<u8>, chunk: &[u8], limit: usize) {
    if chunk.len() >= limit {
        bytes.clear();
        bytes.extend_from_slice(&chunk[chunk.len() - limit..]);
    } else {
        let excess = bytes
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(limit);
        bytes.drain(..excess);
        bytes.extend_from_slice(chunk);
    }
}
async fn stderr_tail(mut stream: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok(bytes);
        }
        append_tail(&mut bytes, &buffer[..count], limit);
    }
}
impl Host {
    pub fn run(&self, request: CommandRequest) -> Result<Vec<u8>> {
        self.run_output(request).map(|output| output.stdout)
    }
    pub fn run_output(&self, request: CommandRequest) -> Result<CommandOutput> {
        let host = self.clone();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        crate::ssh::runtime().spawn(async move {
            let timeout = request.timeout;
            let result = tokio::time::timeout(timeout, host.execute(request))
                .await
                .with_diagnostic(crate::diagnostic!("host.command_timeout"))
                .and_then(|result| result);
            let _ = sender.send(result);
        });
        receiver
            .recv()
            .with_diagnostic(crate::diagnostic!("ssh.worker_stopped"))?
    }
    async fn execute(&self, request: CommandRequest) -> Result<CommandOutput> {
        match self {
            Self::Local => {
                let mut command = tokio::process::Command::new(&request.program);
                command
                    .args(&request.arguments)
                    .envs(&request.environment)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
                if let Some(directory) = request.directory {
                    let HostPath::Local(directory) = directory else {
                        anyhow::bail!(crate::diagnostic!("host.path_mismatch"));
                    };
                    command.current_dir(directory);
                }
                #[cfg(windows)]
                command.creation_flags(0x08000000);
                let mut child = command.spawn()?;
                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| anyhow::anyhow!(crate::diagnostic!("host.no_stdio")))?;
                let stderr = child
                    .stderr
                    .take()
                    .ok_or_else(|| anyhow::anyhow!(crate::diagnostic!("host.no_stdio")))?;
                let mut stdin = child.stdin.take();
                let (stdout, stderr, status) = tokio::try_join!(
                    limited(stdout, request.output_limit),
                    stderr_tail(stderr, request.output_limit),
                    async {
                        if let Some(mut stdin) = stdin.take() {
                            stdin.write_all(&request.stdin).await?;
                        }
                        child.wait().await.map_err(anyhow::Error::from)
                    }
                )?;
                ensure!(
                    status
                        .code()
                        .is_some_and(|code| request.allowed_status.contains(&code)),
                    crate::diagnostic!(
                        "ssh.command_failed",
                        output = String::from_utf8_lossy(&stderr).trim()
                    )
                );
                Ok(CommandOutput { stdout, stderr })
            }
            Self::Ssh(config) => {
                let mut channel = crate::ssh::exec_aux(config, &request.remote_command()?).await?;
                if !request.stdin.is_empty() {
                    channel.data(request.stdin.as_slice()).await?;
                }
                channel.eof().await?;
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                let mut status = None;
                while let Some(message) = channel.wait().await {
                    match message {
                        russh::ChannelMsg::Data { data } => {
                            ensure!(
                                stdout.len().saturating_add(data.len()) <= request.output_limit,
                                crate::diagnostic!("host.output_too_large")
                            );
                            stdout.extend_from_slice(&data);
                        }
                        russh::ChannelMsg::ExtendedData { data, .. } => {
                            append_tail(&mut stderr, &data, request.output_limit)
                        }
                        russh::ChannelMsg::ExitStatus { exit_status } => {
                            status = i32::try_from(exit_status).ok()
                        }
                        russh::ChannelMsg::Close => break,
                        _ => {}
                    }
                }
                ensure!(
                    status.is_some_and(|code| request.allowed_status.contains(&code)),
                    crate::diagnostic!(
                        "ssh.command_failed",
                        output = String::from_utf8_lossy(&stderr).trim()
                    )
                );
                Ok(CommandOutput { stdout, stderr })
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn nonzero_status_and_stderr_are_kept_separate() {
        let mut request = CommandRequest::new(
            "cmd.exe",
            ["/D", "/C", "echo stdout&echo stderr 1>&2&exit /B 7"],
        );
        request.allowed_status = vec![7];
        let bytes = Host::Local.run(request).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap().trim(), "stdout");
    }
    #[test]
    fn posix_command_quotes_metacharacters_without_windows_path_rules() {
        let mut request = CommandRequest::new("git", ["show", "a'b;$(echo nope)"]);
        request.directory = Some(HostPath::Remote(crate::PosixPath::new(r"/srv/folder\name")));
        assert_eq!(
            request.remote_command().unwrap(),
            r"cd '/srv/folder\name' && env 'git' 'show' 'a'\''b;$(echo nope)'"
        );
    }
    #[cfg(windows)]
    #[test]
    fn successful_command_retains_stderr_output_without_an_eight_kib_failure() {
        let request = CommandRequest::new(
            "cmd.exe",
            [
                "/D",
                "/C",
                "echo stdout&for /L %i in (1,1,3000) do @echo stderr-%i 1>&2",
            ],
        );
        let output = Host::Local.run_output(request).unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "stdout");
        assert!(output.stderr.len() > 8192);
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("stderr-3000")
        );
    }
}
