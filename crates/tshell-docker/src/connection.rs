use anyhow::Result;
use bollard::{API_DEFAULT_VERSION, Docker};
use serde::{Deserialize, Serialize};
use tshell_host::{HostId, SshConfig};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Endpoint {
    Local { path: Option<String> },
    Ssh(SshConfig),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub id: HostId,
    pub name: String,
    pub endpoint: Endpoint,
}
impl Target {
    pub fn local(name: String, path: Option<String>) -> Self {
        Self {
            id: HostId::local(),
            name,
            endpoint: Endpoint::Local { path },
        }
    }
    pub async fn connect(&self) -> Result<Docker> {
        let docker = match &self.endpoint {
            Endpoint::Local { path } => {
                #[cfg(windows)]
                {
                    Docker::connect_with_named_pipe(
                        path.as_deref().unwrap_or("//./pipe/docker_engine"),
                        30,
                        API_DEFAULT_VERSION,
                    )?
                }
                #[cfg(unix)]
                {
                    Docker::connect_with_unix(
                        path.as_deref().unwrap_or("/var/run/docker.sock"),
                        30,
                        API_DEFAULT_VERSION,
                    )?
                }
            }
            Endpoint::Ssh(host) => {
                let host = host.clone();
                let auxiliary = std::sync::Arc::new(tshell_host::ssh::auxiliary(&host));
                Docker::connect_with_custom_transport(
                    move |request: bollard::BollardRequest| {
                        let host = host.clone();
                        let auxiliary = auxiliary.clone();
                        async move {
                            let _auxiliary = auxiliary;
                            let channel = if request.headers().contains_key(http::header::UPGRADE) {
                                let mut channel =
                                    tshell_host::ssh::open(&host).await.map_err(|error| {
                                        std::io::Error::other(error.into_boxed_dyn_error())
                                    })?;
                                channel
                                    .exec(true, "docker system dial-stdio")
                                    .await
                                    .map_err(std::io::Error::other)?;
                                tshell_host::ssh::confirmed(&mut channel).await.map_err(
                                    |error| std::io::Error::other(error.into_boxed_dyn_error()),
                                )?;
                                channel
                            } else {
                                tshell_host::ssh::exec_aux(&host, "docker system dial-stdio")
                                    .await
                                    .map_err(|error| {
                                        std::io::Error::other(error.into_boxed_dyn_error())
                                    })?
                            };
                            request_over_stream(channel.into_stdio(), request).await
                        }
                    },
                    Some("http://docker"),
                    30,
                    API_DEFAULT_VERSION,
                )?
            }
        };
        Ok(docker.negotiate_version().await?)
    }
}

struct PendingConnection(Option<tokio::task::AbortHandle>);
impl Drop for PendingConnection {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}
// The SDK owns API requests and hijacked exec IO; this adapter only transports HTTP.
async fn request_over_stream(
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    mut request: bollard::BollardRequest,
) -> Result<http::Response<hyper::body::Incoming>, bollard::errors::Error> {
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await?;
    let task = tshell_host::ssh::runtime().spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    let mut pending = PendingConnection(Some(task.abort_handle()));
    if let Some(path) = request.uri().path_and_query().cloned() {
        *request.uri_mut() = path
            .as_str()
            .parse::<http::Uri>()
            .map_err(|error| std::io::Error::other(error))?;
    }
    request
        .headers_mut()
        .insert(http::header::HOST, http::HeaderValue::from_static("docker"));
    let response = sender.send_request(request).await?;
    // Response bodies and upgraded streams now own transport cancellation.
    pending.0 = None;
    Ok(response)
}
#[cfg(test)]
mod tests;
