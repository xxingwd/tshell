use crate::{Endpoint, Target, resources::host};
use anyhow::{Result, ensure};
use std::time::Duration;
use tshell_host::{CommandRequest, Host};

#[derive(Clone)]
pub(crate) struct Binding {
    pub host: Host,
    pub arguments: Vec<String>,
    pub engine_id: String,
}
impl Binding {
    pub fn resolve(target: &Target, engine_id: &str) -> Result<Self> {
        ensure!(
            !engine_id.is_empty(),
            crate::t!("docker.engine_identity_missing").into_owned()
        );
        let host = host(target);
        let arguments = arguments(target)?;
        let binding = Self {
            host,
            arguments,
            engine_id: engine_id.into(),
        };
        binding.verify()?;
        Ok(binding)
    }
    pub fn request(&self, arguments: impl IntoIterator<Item = String>) -> CommandRequest {
        let mut request =
            CommandRequest::new("docker", self.arguments.iter().cloned().chain(arguments));
        request.output_limit = 2 * 1024 * 1024;
        request.timeout = Duration::from_secs(600);
        request
    }
    pub fn verify(&self) -> Result<()> {
        let mut request = self.request(["info".into(), "--format".into(), "{{json .ID}}".into()]);
        request.timeout = Duration::from_secs(30);
        let output = self.host.run(request)?;
        let id: String = serde_json::from_slice(&output)?;
        ensure!(
            id == self.engine_id,
            crate::t!("docker.cli_engine_mismatch").into_owned()
        );
        Ok(())
    }
    pub fn daemon_endpoint(&self) -> Result<String> {
        if self.arguments.first().map(String::as_str) == Some("--host") {
            return Ok(self.arguments[1].clone());
        }
        let output = self.host.run(tshell_host::CommandRequest::new(
            "docker",
            [
                "context",
                "inspect",
                self.arguments[1].as_str(),
                "--format",
                "{{json .Endpoints.docker.Host}} ",
            ],
        ))?;
        Ok(serde_json::from_slice(&output)?)
    }
    pub fn run(&self, request: CommandRequest) -> Result<String> {
        self.verify()?;
        let output = self.host.run_output(request)?;
        Ok(output_text(output))
    }
}
pub(crate) fn output_text(output: tshell_host::command::CommandOutput) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    text
}
fn local_endpoint(path: Option<&str>) -> String {
    let path = path.unwrap_or(if cfg!(windows) {
        "//./pipe/docker_engine"
    } else {
        "/var/run/docker.sock"
    });
    if cfg!(windows) {
        format!("npipe://{}", path.replace('\\', "/"))
    } else {
        format!("unix://{path}")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_local_endpoint_is_explicit_and_does_not_use_the_cli_context() {
        if cfg!(windows) {
            assert_eq!(
                local_endpoint(Some(r"\\.\pipe\engine-with-spaces")),
                "npipe:////./pipe/engine-with-spaces"
            );
        } else {
            assert_eq!(
                local_endpoint(Some("/tmp/docker.sock")),
                "unix:///tmp/docker.sock"
            );
        }
    }
}

pub(crate) fn arguments(target: &Target) -> Result<Vec<String>> {
    let host = host(target);
    Ok(match &target.endpoint {
        Endpoint::Local { path } => vec!["--host".into(), local_endpoint(path.as_deref())],
        Endpoint::Ssh(_) => {
            let output = host.run(CommandRequest::new("sh", ["-c", "if [ -n \"$DOCKER_CONTEXT\" ]; then printf 'context\\n%s\\n' \"$DOCKER_CONTEXT\"; elif [ -n \"$DOCKER_HOST\" ]; then printf 'host\\n%s\\n' \"$DOCKER_HOST\"; else printf 'context\\n'; docker context show; fi"]))?;
            let value = String::from_utf8(output)?;
            let (kind, name) = value.trim().split_once('\n').ok_or_else(|| {
                anyhow::anyhow!(crate::t!("docker.cli_target_invalid").into_owned())
            })?;
            ensure!(
                matches!(kind, "context" | "host") && !name.trim().is_empty(),
                crate::t!("docker.cli_target_invalid").into_owned()
            );
            vec![format!("--{kind}"), name.trim().into()]
        }
    })
}
