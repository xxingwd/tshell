use super::host_metrics::Target;
use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    time::timeout,
};

const LINUX: &str = include_str!("metrics_linux.sh");
const WINDOWS: &str = include_str!("metrics_windows.ps1");

#[derive(Clone, Copy)]
pub(super) enum Platform {
    Linux,
    Windows,
}
impl Platform {
    fn arguments(self) -> (&'static str, Vec<String>) {
        match self {
            Self::Linux => ("sh", vec!["-s".into()]),
            Self::Windows => {
                let encoded = STANDARD.encode(
                    "[Console]::InputEncoding=[Text.Encoding]::UTF8; & ([ScriptBlock]::Create([Console]::In.ReadToEnd()))"
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect::<Vec<_>>(),
                );
                (
                    "powershell.exe",
                    vec![
                        "-NoLogo".into(),
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-EncodedCommand".into(),
                        encoded,
                    ],
                )
            }
        }
    }
}

pub(super) async fn detect(target: &Target) -> Result<Platform> {
    match target {
        Target::Local => {
            if cfg!(windows) {
                Ok(Platform::Windows)
            } else if cfg!(target_os = "linux") {
                Ok(Platform::Linux)
            } else {
                bail!("{}", crate::t!("metrics.transport_unsupported"))
            }
        }
        Target::Remote(host) => {
            let channel = crate::ssh_pool::exec_aux(host, "uname -s").await?;
            let mut output = Vec::new();
            timeout(
                Duration::from_secs(20),
                channel.into_stream().take(4096).read_to_end(&mut output),
            )
            .await??;
            match String::from_utf8_lossy(&output).trim() {
                "Linux" => Ok(Platform::Linux),
                "Darwin" | "FreeBSD" | "OpenBSD" => {
                    bail!("{}", crate::t!("metrics.transport_unsupported"))
                }
                // Windows' default SSH shell may be cmd or PowerShell; both accept EncodedCommand.
                _ => Ok(Platform::Windows),
            }
        }
    }
}

pub(super) struct Connection {
    pub reader: Box<dyn AsyncRead + Unpin + Send>,
    // Dropping the monitor kills its local collector; SSH streams close their channel on drop.
    _child: Option<Child>,
}
pub(super) async fn connect(
    target: &Target,
    platform: Platform,
    metrics: &std::collections::BTreeSet<super::metrics_config::Metric>,
) -> Result<Connection> {
    let (program, args) = platform.arguments();
    let script = script(platform, metrics);
    match target {
        Target::Remote(host) => {
            let command = format!("{program} {}", args.join(" "));
            let channel = crate::ssh_pool::exec_aux(host, &command).await?;
            channel.data(script.as_bytes()).await?;
            channel.eof().await?;
            Ok(Connection {
                reader: Box::new(channel.into_stream()),
                _child: None,
            })
        }
        Target::Local => {
            let mut command = Command::new(program);
            command
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            #[cfg(windows)]
            {
                command.creation_flags(0x08000000);
            }
            let mut child = command.spawn()?;
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("{}", crate::t!("metrics.script_write_failed")))?;
            stdin.write_all(script.as_bytes()).await?;
            stdin.shutdown().await?;
            drop(stdin);
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("{}", crate::t!("metrics.script_read_failed")))?;
            Ok(Connection {
                reader: Box::new(stdout),
                _child: Some(child),
            })
        }
    }
}

fn script(
    platform: Platform,
    metrics: &std::collections::BTreeSet<super::metrics_config::Metric>,
) -> String {
    use super::metrics_config::Metric;
    let groups = [
        ("cpu", metrics.contains(&Metric::Cpu)),
        ("memory", metrics.contains(&Metric::Memory)),
        (
            "network",
            metrics.contains(&Metric::Download) || metrics.contains(&Metric::Upload),
        ),
        ("disk", metrics.contains(&Metric::Disk)),
        ("io", metrics.contains(&Metric::DiskIo)),
        ("temperature", metrics.contains(&Metric::Temperature)),
    ];
    let mut script = String::new();
    for (name, enabled) in groups {
        match platform {
            Platform::Linux => script.push_str(&format!("collect_{name}={}\n", u8::from(enabled))),
            Platform::Windows => script.push_str(&format!("$collect_{name}=${enabled}\n")),
        }
    }
    script.push_str(match platform {
        Platform::Linux => LINUX,
        Platform::Windows => WINDOWS,
    });
    script.replace("\r\n", "\n")
}
