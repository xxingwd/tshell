//! One lazy SFTP channel owned by a workspace session; no idle timer.
use super::*;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Session {
    pub host: HostConfig,
    client: Arc<tokio::sync::Mutex<Option<sftp::Client>>>,
}
impl Session {
    pub fn new(host: HostConfig) -> Self {
        Self {
            host,
            client: Arc::default(),
        }
    }
}
impl std::ops::Deref for Session {
    type Target = HostConfig;
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
        .context(crate::t!("sftp.queue_timeout"))?;
    if cached.is_none() {
        *cached = Some(sftp::connect(&session.host).await?);
    }
    let result = tokio::time::timeout(Duration::from_secs(60), operation(cached.as_ref().unwrap()))
        .await
        .context(crate::t!("sftp.op_timeout"))
        .and_then(|result| result);
    // Drop a broken protocol session, but never replay a failed operation (especially writes).
    if result.as_ref().err().is_some_and(|e| {
        e.downcast_ref::<tokio::time::error::Elapsed>().is_some()
            || matches!(
                e.downcast_ref::<russh_sftp::client::error::Error>(),
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
    result.context(crate::t!("sftp.op_failed"))
}
