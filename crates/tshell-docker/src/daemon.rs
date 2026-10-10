use crate::{Target, cli::Binding, documents, resources::host};
use anyhow::{Result, ensure};
use bollard::models::SystemInfo;
use serde_json::Value;
use tshell_host::{CommandRequest, Host};

#[derive(Clone, Debug)]
pub(crate) enum Manager {
    Desktop,
    System { user: bool, reload: bool },
    Windows,
    Manual,
}
#[derive(Clone, Debug)]
pub(crate) struct Source {
    pub path: String,
    pub manager: Manager,
    pub engine_id: Option<String>,
}
pub(crate) fn discover(target: &Target, info: &SystemInfo) -> Result<Source> {
    let host = host(target);
    let mut remote_endpoint = None;
    if matches!(host, Host::Ssh(_)) {
        let binding = Binding::resolve(target, info.id.as_deref().unwrap_or_default())?;
        let endpoint = binding.daemon_endpoint()?;
        ensure!(
            endpoint.starts_with("unix://"),
            crate::t!("docker.config_source_required").into_owned()
        );
        remote_endpoint = Some(endpoint);
    }
    let desktop = info
        .operating_system
        .as_deref()
        .is_some_and(|os| os.to_ascii_lowercase().contains("docker desktop"));
    let rootless = info
        .security_options
        .as_ref()
        .is_some_and(|options| options.iter().any(|value| value.contains("rootless")));
    let (path, manager) = if desktop {
        ensure!(
            matches!(host, Host::Local),
            crate::t!("docker.config_source_required").into_owned()
        );
        let home = home(&host)?;
        (
            if matches!(host, Host::Local) {
                std::path::PathBuf::from(home)
                    .join(".docker")
                    .join("daemon.json")
                    .to_string_lossy()
                    .into_owned()
            } else {
                format!("{home}/.docker/daemon.json")
            },
            Manager::Desktop,
        )
    } else if info.os_type.as_deref() == Some("windows") {
        ensure!(
            matches!(host, Host::Local),
            crate::t!("docker.config_source_required").into_owned()
        );
        ensure!(
            matches!(&target.endpoint, crate::Endpoint::Local { path: None })
                || matches!(&target.endpoint, crate::Endpoint::Local {path:Some(path)} if path.replace('\\',"/") == "//./pipe/docker_engine"),
            crate::t!("docker.config_source_required").into_owned()
        );
        let base = std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into());
        let service =
            String::from_utf8_lossy(&host.run(CommandRequest::new("sc.exe", ["qc", "docker"]))?)
                .into_owned();
        let command = service
            .lines()
            .find(|line| {
                let line = line.to_ascii_lowercase();
                line.contains("dockerd") || line.contains("docker.exe")
            })
            .ok_or_else(|| {
                anyhow::anyhow!(crate::t!("docker.config_source_required").into_owned())
            })?;
        (
            windows_config_path(command).unwrap_or_else(|| {
                std::path::PathBuf::from(base)
                    .join("docker/config/daemon.json")
                    .to_string_lossy()
                    .into_owned()
            }),
            Manager::Windows,
        )
    } else {
        ensure!(
            info.os_type.as_deref() == Some("linux"),
            crate::t!("docker.config_source_required").into_owned()
        );
        ensure!(
            !(cfg!(windows) && matches!(host, Host::Local)),
            crate::t!("docker.config_source_required").into_owned()
        );
        let mut arguments = vec![];
        if rootless {
            arguments.push("--user");
        }
        arguments.extend(["show", "docker.service", "--property=ExecStart", "--value"]);
        let service = host.run(CommandRequest::new("systemctl", arguments));
        let socket = match remote_endpoint {
            Some(endpoint) => endpoint.trim_start_matches("unix://").to_owned(),
            None => match &target.endpoint {
                crate::Endpoint::Local { path } => path
                    .clone()
                    .unwrap_or_else(|| "/var/run/docker.sock".into()),
                _ => unreachable!(),
            },
        };
        let default_socket = if rootless {
            let user = String::from_utf8(host.run(CommandRequest::new("id", ["-u"]))?)?;
            format!("/run/user/{}/docker.sock", user.trim())
        } else {
            "/var/run/docker.sock".into()
        };
        ensure!(
            socket == default_socket
                || (!rootless && socket == "/run/docker.sock")
                || service
                    .as_ref()
                    .is_ok_and(|bytes| String::from_utf8_lossy(bytes)
                        .contains(&format!("unix://{socket}"))),
            crate::t!("docker.config_source_required").into_owned()
        );
        let mut unit_arguments = vec![];
        if rootless {
            unit_arguments.push("--user");
        }
        let mut active_arguments = unit_arguments.clone();
        active_arguments.extend(["is-active", "docker.service"]);
        let active = service.is_ok()
            && host
                .run(CommandRequest::new("systemctl", active_arguments))
                .is_ok();
        let mut reload_arguments = unit_arguments;
        reload_arguments.extend(["show", "docker.service", "--property=CanReload", "--value"]);
        let reload = active
            && host
                .run(CommandRequest::new("systemctl", reload_arguments))
                .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).trim() == "yes");
        let custom = service
            .as_ref()
            .ok()
            .and_then(|bytes| config_path(&String::from_utf8_lossy(bytes)));
        let custom = if active {
            custom
        } else {
            let processes = String::from_utf8(
                host.run(CommandRequest::new("ps", ["-C", "dockerd", "-o", "args="]))?,
            )?;
            let processes: Vec<_> = processes
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            ensure!(
                processes.len() == 1,
                crate::t!("docker.config_source_required").into_owned()
            );
            config_path(processes[0])
        };
        let path = if let Some(custom) = custom {
            custom
        } else if rootless {
            let config = match &host {
                Host::Local => std::env::var("XDG_CONFIG_HOME")
                    .ok()
                    .filter(|value| !value.is_empty()),
                Host::Ssh(_) => host
                    .run(CommandRequest::new(
                        "sh",
                        ["-c", "printf '%s' \"$XDG_CONFIG_HOME\""],
                    ))
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .filter(|value| !value.is_empty()),
            };
            format!(
                "{}/docker/daemon.json",
                config.unwrap_or(format!("{}/.config", home(&host)?))
            )
        } else {
            "/etc/docker/daemon.json".into()
        };
        (
            path,
            if active {
                Manager::System {
                    user: rootless,
                    reload,
                }
            } else {
                Manager::Manual
            },
        )
    };
    Ok(Source {
        path,
        manager,
        engine_id: info.id.clone(),
    })
}
fn home(host: &Host) -> Result<String> {
    match host {
        Host::Local => {
            std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map_err(Into::into)
        }
        Host::Ssh(_) => Ok(String::from_utf8(
            host.run(CommandRequest::new("sh", ["-c", "printf '%s' \"$HOME\""]))?,
        )?),
    }
}
fn config_path(command: &str) -> Option<String> {
    let words = shell_words::split(command).ok()?;
    words.iter().enumerate().find_map(|(index, word)| {
        word.strip_prefix("--config-file=")
            .map(str::to_owned)
            .or_else(|| {
                if word == "--config-file" {
                    words.get(index + 1).cloned()
                } else {
                    None
                }
            })
    })
}
fn windows_config_path(command: &str) -> Option<String> {
    let value = command.split_once("--config-file")?.1;
    let value = value
        .strip_prefix('=')
        .or_else(|| value.strip_prefix(' '))?
        .trim_start();
    if let Some(value) = value.strip_prefix('"') {
        Some(value.split_once('"')?.0.into())
    } else {
        value.split_whitespace().next().map(str::to_owned)
    }
}
pub(crate) fn validate_json(text: &str) -> Result<()> {
    let value: Value = serde_json::from_str(text)?;
    ensure!(
        value.is_object(),
        crate::t!("docker.json_object_required").into_owned()
    );
    Ok(())
}
pub(crate) fn validate(target: &Target, source: &Source, text: &str) -> Result<String> {
    validate_json(text)?;
    let host = host(target);
    let available = match &host {
        Host::Local if cfg!(windows) => host.run(CommandRequest::new("where.exe", ["dockerd.exe"])),
        _ => host.run(CommandRequest::new("sh", ["-c", "command -v dockerd"])),
    };
    if available.is_err() || matches!(source.manager, Manager::Desktop) {
        return Ok(crate::t!("docker.json_validated").into_owned());
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let path = match &host {
        Host::Local => std::env::temp_dir()
            .join(format!("tshell-daemon-{nonce}.json"))
            .to_string_lossy()
            .into_owned(),
        Host::Ssh(_) => String::from_utf8(host.run(CommandRequest::new(
            "mktemp",
            ["/tmp/tshell-daemon-XXXXXXXX.json"],
        ))?)?
        .trim()
        .to_owned(),
    };
    let prepared = (|| {
        let file = documents::read(target, path.clone())?;
        let saved = documents::save(target, &file, text)?;
        if let Some(backup) = saved.backup {
            documents::remove_temporary(target, &backup);
        }
        host.run(CommandRequest::new(
            "dockerd",
            ["--validate", "--config-file", path.as_str()],
        ))?;
        Ok(crate::t!("docker.config_validated").into_owned())
    })();
    match host {
        Host::Local => {
            let _ = std::fs::remove_file(&path);
        }
        Host::Ssh(_) => {
            let _ = host.run(CommandRequest::new("rm", ["--", path.as_str()]));
        }
    }
    prepared
}
pub(crate) fn apply(
    target: &Target,
    engine_id: &str,
    source: &Source,
    restart: bool,
) -> Result<String> {
    ensure!(
        source.engine_id.as_deref() == Some(engine_id),
        crate::t!("docker.config_source_required").into_owned()
    );
    let binding = Binding::resolve(target, engine_id)?;
    match &source.manager {
        Manager::Desktop => {
            ensure!(restart, crate::t!("docker.restart_required").into_owned());
            let mut request =
                CommandRequest::new("docker", ["desktop", "restart", "--timeout", "120"]);
            request.timeout = std::time::Duration::from_secs(150);
            binding.host.run(request)?;
        }
        Manager::System { user, reload } => {
            ensure!(
                restart || *reload,
                crate::t!("docker.restart_required").into_owned()
            );
            let mut arguments = vec![];
            if *user {
                arguments.push("--user");
            }
            arguments.extend([if restart { "restart" } else { "reload" }, "docker.service"]);
            let mut request = CommandRequest::new("systemctl", arguments);
            request.timeout = std::time::Duration::from_secs(120);
            binding.host.run(request)?;
        }
        Manager::Windows => {
            ensure!(restart, crate::t!("docker.restart_required").into_owned());
            binding
                .host
                .run(CommandRequest::new("sc.exe", ["stop", "docker"]))?;
            for _ in 0..30 {
                let status = binding
                    .host
                    .run(CommandRequest::new("sc.exe", ["query", "docker"]))?;
                if String::from_utf8_lossy(&status).contains("STOPPED") {
                    binding
                        .host
                        .run(CommandRequest::new("sc.exe", ["start", "docker"]))?;
                    return Ok(crate::t!("docker.config_applied").into_owned());
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            anyhow::bail!(crate::t!("host.command_timeout").into_owned());
        }
        Manager::Manual => anyhow::bail!(crate::t!("docker.config_source_required").into_owned()),
    }
    Ok(crate::t!("docker.config_applied").into_owned())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn daemon_config_requires_an_object_without_dropping_unknown_keys() {
        assert!(validate_json("[]").is_err());
        assert!(
            validate_json(r#"{"registry-mirrors":["https://mirror.example"],"future-key":true}"#)
                .is_ok()
        );
    }
    #[test]
    fn custom_config_path_retains_spaces() {
        assert_eq!(
            config_path("/usr/bin/dockerd --config-file '/srv/docker config.json'"),
            Some("/srv/docker config.json".into())
        );
        assert_eq!(
            windows_config_path(
                r#"BINARY_PATH_NAME : "C:\Program Files\Docker\dockerd.exe" --run-service --config-file="D:\Docker Config\daemon.json""#
            ),
            Some(r"D:\Docker Config\daemon.json".into())
        );
    }
}
