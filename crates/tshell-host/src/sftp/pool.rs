//! One lazy SFTP channel owned by a workspace session; no idle timer.
use super::*;
use crate::diagnostic::ResultExt;
use std::sync::Arc;

#[derive(Clone)]
pub struct Session {
    pub host: SshConfig,
    client: Arc<tokio::sync::Mutex<Option<sftp::Client>>>,
}
impl Session {
    pub fn new(host: SshConfig) -> Self {
        Self {
            host,
            client: Arc::default(),
        }
    }
}
impl std::ops::Deref for Session {
    type Target = SshConfig;
    fn deref(&self) -> &Self::Target {
        &self.host
    }
}

pub(super) async fn run<T>(
    session: Session,
    operation: impl for<'a> FnOnce(&'a sftp::Client) -> Operation<'a, T>,
) -> Result<T> {
    let mut cached = tokio::time::timeout(Duration::from_secs(60), session.client.lock())
        .await
        .with_diagnostic(crate::diagnostic!("sftp.queue_timeout"))?;
    if cached.is_none() {
        *cached = Some(sftp::connect(&session.host).await?);
    }
    let result = tokio::time::timeout(Duration::from_secs(60), operation(cached.as_ref().unwrap()))
        .await
        .with_diagnostic(crate::diagnostic!("sftp.op_timeout"))
        .and_then(|result| result);
    // Drop a broken protocol session, but never replay a failed operation (especially writes).
    if result.as_ref().err().is_some_and(|e| {
        crate::diagnostic::find_cause::<tokio::time::error::Elapsed>(e).is_some()
            || matches!(
                crate::diagnostic::find_cause::<russh_sftp::client::error::Error>(e),
                Some(
                    russh_sftp::client::error::Error::IO(_)
                        | russh_sftp::client::error::Error::Timeout
                        | russh_sftp::client::error::Error::UnexpectedBehavior(_)
                        | russh_sftp::client::error::Error::UnexpectedPacket
                )
            )
    }) {
        cached.take();
    }
    result.with_diagnostic(crate::diagnostic!("sftp.op_failed"))
}
