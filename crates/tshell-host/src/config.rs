use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, process::Command};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshConfig {
    pub destination: String,
    #[serde(default)]
    pub user: String,
    pub port: Option<u16>,
    #[serde(default)]
    pub identity_file: Option<PathBuf>,
}
impl SshConfig {
    pub fn ssh_command(&self) -> Result<Command> {
        validate_destination(&self.destination)?;
        let mut command = Command::new("ssh");
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
        ]);
        if let Some(port) = self.port {
            command.args(["-p", &port.to_string()]);
        }
        if let Some(identity_file) = &self.identity_file {
            command.arg("-i").arg(identity_file);
        }
        command.arg("--").arg(&self.destination);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        Ok(command)
    }
}

pub fn validate_destination(value: &str) -> Result<String> {
    let value = value.trim();
    anyhow::ensure!(
        !value.is_empty(),
        crate::diagnostic!("term.destination_required")
    );
    anyhow::ensure!(
        !value.starts_with('-') && !value.chars().any(|c| c.is_whitespace() || c.is_control()),
        crate::diagnostic!("term.destination_whitespace")
    );
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_command_uses_selected_identity_file() {
        let host = SshConfig {
            destination: "server".into(),
            identity_file: Some("key with spaces".into()),
            ..Default::default()
        };
        let command = host.ssh_command().unwrap();
        let args: Vec<_> = command.get_args().collect();
        assert!(
            args.windows(2)
                .any(|args| args == ["-i", "key with spaces"])
        );
    }
}
