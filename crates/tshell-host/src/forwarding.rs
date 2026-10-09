//! SSH forwarding lifecycle, payload counters, and persisted rules.

use crate::SshConfig;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum ForwardKind {
    #[default]
    Local,
    Remote,
    Dynamic,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LocalForward {
    #[serde(default)]
    pub kind: ForwardKind,
    pub bind_host: String,
    pub local_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

impl LocalForward {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            valid_host(&self.bind_host),
            crate::diagnostic!("ssh.forward_invalid_bind")
        );
        ensure!(
            self.local_port > 0,
            crate::diagnostic!("ssh.forward_invalid_port")
        );
        if self.kind != ForwardKind::Dynamic {
            ensure!(
                valid_host(&self.target_host),
                crate::diagnostic!("ssh.forward_invalid_target")
            );
            ensure!(
                self.target_port > 0,
                crate::diagnostic!("ssh.forward_invalid_port")
            );
        }
        Ok(())
    }

    pub fn label(&self) -> String {
        match self.kind {
            ForwardKind::Dynamic => format!("{}:{}", self.bind_host, self.local_port),
            ForwardKind::Local | ForwardKind::Remote => format!(
                "{}:{} → {}:{}",
                self.bind_host, self.local_port, self.target_host, self.target_port
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Running,
    Failed(crate::Failure),
    Stopped,
}

#[derive(Clone)]
pub struct Statistics {
    phase: std::sync::Arc<parking_lot::Mutex<Phase>>,
    pub sent: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub received: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl Statistics {
    pub fn phase(&self) -> Phase {
        self.phase.lock().clone()
    }
    pub fn new() -> Self {
        Self {
            phase: std::sync::Arc::new(parking_lot::Mutex::new(Phase::Connecting)),
            sent: Default::default(),
            received: Default::default(),
        }
    }
    pub fn set_phase(&self, phase: Phase) {
        *self.phase.lock() = phase;
    }
    pub fn counters(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering;
        (
            self.sent.load(Ordering::Relaxed),
            self.received.load(Ordering::Relaxed),
        )
    }
}

pub struct Tunnel {
    cancel: tokio::sync::watch::Sender<bool>,
    stats: Statistics,
}

impl Tunnel {
    pub fn start(
        host: &SshConfig,
        forward: &LocalForward,
        updates: async_channel::Sender<()>,
    ) -> Result<Self> {
        forward.validate()?;
        crate::config::validate_destination(&host.destination)?;
        let (cancel, cancelled) = tokio::sync::watch::channel(false);
        let stats = Statistics::new();
        let worker_stats = stats.clone();
        let host = host.clone();
        let forward = forward.clone();
        crate::ssh::runtime().spawn(async move {
            let result = crate::ssh::forwarding::serve(
                &host,
                &forward,
                worker_stats.clone(),
                updates.clone(),
                cancelled.clone(),
            )
            .await;
            let phase = if *cancelled.borrow() {
                Phase::Stopped
            } else {
                match result {
                    Ok(()) => Phase::Stopped,
                    Err(error) => Phase::Failed(crate::Failure::from(&error)),
                }
            };
            worker_stats.set_phase(phase);
            let _ = updates.try_send(());
        });
        Ok(Self { cancel, stats })
    }

    pub fn phase(&self) -> Phase {
        self.stats.phase()
    }
    pub fn counters(&self) -> (u64, u64) {
        self.stats.counters()
    }
    pub fn is_active(&self) -> bool {
        matches!(self.phase(), Phase::Connecting | Phase::Running)
    }
    pub fn stop(&mut self) {
        let _ = self.cancel.send(true);
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stop();
    }
}
fn valid_host(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .chars()
            .all(|character| !character.is_whitespace() && !character.is_control())
}

#[cfg(test)]
mod tests {
    use super::{ForwardKind, LocalForward};

    #[test]
    fn validates_and_labels_local_forward() {
        let forward = LocalForward {
            kind: ForwardKind::Local,
            bind_host: "127.0.0.1".into(),
            local_port: 8080,
            target_host: "localhost".into(),
            target_port: 80,
        };
        forward.validate().unwrap();
        assert_eq!(forward.label(), "127.0.0.1:8080 → localhost:80");
    }

    #[test]
    fn rejects_shell_like_hosts_and_zero_ports() {
        let mut forward = LocalForward {
            kind: ForwardKind::Local,
            bind_host: "127.0.0.1".into(),
            local_port: 0,
            target_host: "localhost".into(),
            target_port: 80,
        };
        assert!(forward.validate().is_err());
        forward.local_port = 8080;
        forward.target_host = "bad host".into();
        assert!(forward.validate().is_err());
    }

    #[test]
    fn old_local_rule_json_defaults_to_local_forwarding() {
        let forward: LocalForward = serde_json::from_str(
            r#"{"bind_host":"127.0.0.1","local_port":8080,"target_host":"localhost","target_port":80}"#,
        )
        .unwrap();
        assert_eq!(forward.kind, ForwardKind::Local);
    }
}
