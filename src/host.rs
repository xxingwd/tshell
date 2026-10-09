//! Application host preferences; transport configuration lives in tshell-host.
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostDefinition {
    #[serde(flatten)]
    pub ssh: tshell_host::SshConfig,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_tmux")]
    pub tmux: bool,
    #[serde(skip)]
    pub socket: Option<String>,
}
fn default_tmux() -> bool {
    true
}
impl HostDefinition {
    pub(crate) fn tmux(&self) -> Result<String> {
        if let Some(socket) = &self.socket {
            anyhow::ensure!(
                socket
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "invalid socket name"
            );
            Ok(format!("tmux -u -L {socket}"))
        } else {
            Ok("tmux -u".into())
        }
    }
}
