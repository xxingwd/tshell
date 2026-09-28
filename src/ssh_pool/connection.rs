use crate::tmux_client::HostConfig;
use anyhow::{Context, Result, bail, ensure};
use russh::{
    MethodKind, MethodSet,
    client::{self, KeyboardInteractiveAuthResponse},
    keys::{self, PrivateKeyWithHashAlg, PublicKeyOrCertificate},
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    process::Command,
};

#[derive(Clone)]
struct Profile {
    values: BTreeMap<String, Vec<String>>,
    direct: bool,
}
impl Profile {
    fn direct(host: &HostConfig) -> Result<Self> {
        let known_hosts = dirs::home_dir()
            .context(crate::t!("ssh.known_hosts_missing"))?
            .join(".ssh")
            .join("known_hosts");
        let mut values = BTreeMap::new();
        for (key, value) in [
            ("hostname", host.destination.clone()),
            ("user", host.user.clone()),
            ("port", host.port.unwrap_or(22).to_string()),
            (
                "userknownhostsfile",
                known_hosts.to_string_lossy().into_owned(),
            ),
            ("stricthostkeychecking", "ask".into()),
        ] {
            values.insert(key.into(), vec![value]);
        }
        if let Some(path) = &host.identity_file {
            values.insert(
                "identityfile".into(),
                vec![path.to_string_lossy().into_owned()],
            );
        }
        Ok(Self {
            values,
            direct: true,
        })
    }

    async fn load(host: &HostConfig) -> Result<Self> {
        let command = host.ssh_command()?;
        // -G must precede -- and the destination.
        let args: Vec<_> = command.get_args().map(|arg| arg.to_owned()).collect();
        let mut query = std::process::Command::new(command.get_program());
        query.arg("-G").args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            query.creation_flags(0x08000000);
        }
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            Command::from(query).kill_on_drop(true).output(),
        )
        .await??;
        ensure!(
            output.status.success(),
            "{}",
            crate::t!(
                "ssh.config_parse_failed",
                error = String::from_utf8_lossy(&output.stderr)
            )
        );
        let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for line in String::from_utf8(output.stdout)?.lines() {
            if let Some((key, value)) = line.split_once(' ') {
                values.entry(key.into()).or_default().push(value.into());
            }
        }
        Ok(Self {
            values,
            direct: false,
        })
    }
    fn get(&self, key: &str) -> &str {
        self.values
            .get(key)
            .and_then(|values| values.first())
            .map(String::as_str)
            .unwrap_or("")
    }
    fn paths(&self, key: &str) -> Vec<PathBuf> {
        self.values
            .get(key)
            .into_iter()
            .flatten()
            .map(|value| expand(value.trim_matches('"')))
            .collect()
    }

    fn known_hosts_files(&self) -> Vec<PathBuf> {
        if self.direct {
            vec![PathBuf::from(self.get("userknownhostsfile"))]
        } else {
            ["userknownhostsfile", "globalknownhostsfile"]
                .into_iter()
                .flat_map(|name| self.get(name).split_whitespace())
                .map(expand)
                .collect()
        }
    }
}
fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir().unwrap_or_default().join(rest)
    } else {
        PathBuf::from(path)
    }
}

pub(super) struct Handler {
    profile: Profile,
}
impl client::Handler for Handler {
    type Error = anyhow::Error;
    async fn check_server_key(&mut self, server: &PublicKeyOrCertificate) -> Result<bool> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = server else {
            bail!("{}", crate::t!("ssh.host_cert_unsupported"));
        };
        let p = &self.profile;
        let host = if p.get("hostkeyalias").is_empty() {
            p.get("hostname")
        } else {
            p.get("hostkeyalias")
        };
        let port = p.get("port").parse::<u16>()?;
        let files: Vec<_> = p
            .known_hosts_files()
            .into_iter()
            .filter(|path| path.is_file())
            .collect();
        for file in &files {
            // Reject marker entries we cannot faithfully validate (revocations / host CAs).
            let contents = std::fs::read_to_string(file)?;
            ensure!(
                !contents.lines().any(|line| line.starts_with('@')),
                "{}",
                crate::t!("ssh.known_hosts_markers")
            );
            if keys::check_known_hosts_path(host, port, key, file)? {
                return Ok(true);
            }
        }
        match p.get("stricthostkeychecking") {
            "false" | "no" | "off" => Ok(true),
            "accept-new" => {
                let path = p
                    .get("userknownhostsfile")
                    .split_whitespace()
                    .next()
                    .context(crate::t!("ssh.known_hosts_missing"))?;
                let path = expand(path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                keys::known_hosts::learn_known_hosts_path(host, port, key, path)?;
                Ok(true)
            }
            "ask" if p.direct => {
                let fingerprint = key.fingerprint(keys::HashAlg::Sha256).to_string();
                let accepted = super::interactive::ask(
                    &format!("{host}:{port}"),
                    super::interactive::PromptKind::HostKey { fingerprint },
                )
                .await?
                .is_some();
                ensure!(accepted, "{}", crate::t!("ssh.host_key_rejected"));
                let path = expand(p.get("userknownhostsfile"));
                keys::known_hosts::learn_known_hosts_path(host, port, key, path)?;
                Ok(true)
            }
            _ => bail!("{}", crate::t!("ssh.unknown_host_key")),
        }
    }
}

pub(super) struct Connection {
    pub handle: client::Handle<Handler>,
    _proxy: Option<tokio::process::Child>,
}
trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

pub(super) async fn connect(host: &HostConfig) -> Result<Connection> {
    let direct = !host.user.is_empty();
    if direct {
        if let Some(path) = &host.identity_file {
            ensure!(
                path.is_file(),
                "{}",
                crate::t!("ssh.identity_file_missing", path = path.display())
            );
        }
    }
    let p = if direct {
        Profile::direct(host)?
    } else {
        Profile::load(host).await?
    };
    let user = p.get("user").to_owned();
    let hostname = p.get("hostname").to_owned();
    let port: u16 = p.get("port").parse()?;
    let target = format!("{user}@{hostname}:{port}");
    let mut credentials = if direct {
        Some(super::interactive::credentials(&target).lock_owned().await)
    } else {
        None
    };
    let mut proxy = None;
    let proxy_command = p.get("proxycommand");
    let jumps = p.get("proxyjump");
    let proxy_builder = if !proxy_command.is_empty() && proxy_command != "none" {
        let arguments =
            shell_words::split(proxy_command).context(crate::t!("ssh.proxy_parse_failed"))?;
        let arguments = arguments
            .into_iter()
            .map(|arg| expand_tokens(&arg, &hostname, port, &user))
            .collect::<Result<Vec<_>>>()?;
        let mut command =
            std::process::Command::new(arguments.first().context(crate::t!("ssh.proxy_empty"))?);
        command.args(&arguments[1..]);
        Some(command)
    } else if !jumps.is_empty() && jumps != "none" {
        let (earlier, last) = jumps.rsplit_once(',').unwrap_or(("", jumps));
        let mut command = std::process::Command::new("ssh");
        command.args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]);
        if !earlier.is_empty() {
            command.args(["-J", earlier]);
        }
        let (jump, jump_port) = jump_destination(last)?;
        if let Some(port) = jump_port {
            command.args(["-p", &port.to_string()]);
        }
        command.args(["-W", &format!("{hostname}:{port}"), "--", &jump]);
        Some(command)
    } else {
        None
    };
    let stream: Box<dyn Stream> = if let Some(mut command) = proxy_builder {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = Command::from(command)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context(crate::t!("ssh.proxy_spawn_failed"))?;
        let stream = tokio::io::join(
            child.stdout.take().context(crate::t!("ssh.proxy_stdout"))?,
            child.stdin.take().context(crate::t!("ssh.proxy_stdin"))?,
        );
        proxy = Some(child);
        Box::new(stream)
    } else {
        let socket = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::net::TcpStream::connect((hostname.as_str(), port)),
        )
        .await
        .context(crate::t!("ssh.auth_timeout"))??;
        if let Err(error) = socket.set_nodelay(true) {
            tracing::debug!(%error, "Could not enable TCP_NODELAY for SSH socket");
        }
        Box::new(socket)
    };
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    });
    let mut handle = client::connect_stream(config, stream, Handler { profile: p.clone() }).await?;
    if !direct {
        ensure!(
            p.get("pubkeyauthentication") != "false" && p.get("pubkeyauthentication") != "no",
            "{}",
            crate::t!("ssh.public_key_required")
        );
    }
    let mut methods = MethodSet::client_supported();
    let mut authenticated = false;
    if direct {
        match handle.authenticate_none(&user).await? {
            client::AuthResult::Success => authenticated = true,
            client::AuthResult::Failure {
                remaining_methods, ..
            } => methods = remaining_methods,
        }
    }
    let hash = handle.best_supported_rsa_hash().await?.flatten();
    // Use configured identities first, never log or transmit private key material outside SSH signing.
    for path in p.paths("identityfile") {
        if authenticated {
            break;
        }
        if !path.is_file() {
            continue;
        }
        let key = match keys::load_secret_key(&path, None) {
            Ok(key) => Some(key),
            Err(keys::Error::KeyIsEncrypted) if direct => {
                let answer = super::interactive::ask(
                    &target,
                    super::interactive::PromptKind::Secret {
                        label: crate::t!("ssh.key_passphrase", path = path.display()).to_string(),
                        echo: false,
                    },
                )
                .await?;
                answer
                    .map(|passphrase| keys::load_secret_key(&path, Some(&passphrase)))
                    .transpose()?
            }
            Err(error) if direct => return Err(error.into()),
            Err(_) => None,
        };
        if let Some(key) = key {
            let result = handle
                .authenticate_publickey(&user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await?;
            if let client::AuthResult::Failure {
                remaining_methods, ..
            } = &result
            {
                methods = remaining_methods.clone();
            }
            if result.success() {
                authenticated = true;
                break;
            }
        }
    }
    if !authenticated
        && p.get("identityagent") != "none"
        && (!direct || host.identity_file.is_none())
    {
        #[cfg(windows)]
        let agent = keys::agent::client::AgentClient::connect_named_pipe(
            if p.get("identityagent").is_empty() {
                r"\\.\pipe\openssh-ssh-agent"
            } else {
                p.get("identityagent")
            },
        )
        .await;
        #[cfg(unix)]
        let agent = if p.get("identityagent").is_empty() {
            keys::agent::client::AgentClient::connect_env().await
        } else {
            keys::agent::client::AgentClient::connect_uds(expand(p.get("identityagent"))).await
        };
        if let Ok(mut agent) = agent {
            for identity in agent.request_identities().await? {
                let key = identity.public_key().into_owned();
                if matches!(p.get("identitiesonly"), "yes" | "true") {
                    let allowed = p.paths("identityfile").iter().any(|path| {
                        keys::load_public_key(format!("{}.pub", path.display()))
                            .is_ok_and(|allowed| allowed == key)
                    });
                    if !allowed {
                        continue;
                    }
                }
                let result = handle
                    .authenticate_publickey_with(&user, key, hash, &mut agent)
                    .await?;
                if let client::AuthResult::Failure {
                    remaining_methods, ..
                } = &result
                {
                    methods = remaining_methods.clone();
                }
                if result.success() {
                    authenticated = true;
                    break;
                }
            }
        }
    }
    if !authenticated {
        if let Some(saved_password) = credentials.as_deref_mut() {
            authenticated =
                authenticate_interactive(&mut handle, &user, &target, methods, saved_password)
                    .await?;
        }
    }
    ensure!(authenticated, "{}", crate::t!("ssh.key_auth_failed"));
    Ok(Connection {
        handle,
        _proxy: proxy,
    })
}

async fn authenticate_interactive(
    handle: &mut client::Handle<Handler>,
    user: &str,
    host: &str,
    mut methods: MethodSet,
    saved_password: &mut Option<String>,
) -> Result<bool> {
    if methods.contains(&MethodKind::Password) {
        for attempt in 0..3 {
            let password = match saved_password.take() {
                Some(password) => password,
                None => super::interactive::ask(
                    host,
                    super::interactive::PromptKind::Secret {
                        label: if attempt == 0 {
                            crate::t!("ssh.password_prompt")
                        } else {
                            crate::t!("ssh.password_retry")
                        }
                        .to_string(),
                        echo: false,
                    },
                )
                .await?
                .context(crate::t!("ssh.authentication_cancelled"))?,
            };
            match handle.authenticate_password(user, password.clone()).await? {
                client::AuthResult::Success => {
                    *saved_password = Some(password);
                    return Ok(true);
                }
                client::AuthResult::Failure {
                    remaining_methods,
                    partial_success,
                } => {
                    methods = remaining_methods;
                    if partial_success {
                        *saved_password = Some(password);
                        break;
                    }
                    if !methods.contains(&MethodKind::Password) {
                        break;
                    }
                }
            }
        }
    }
    if methods.contains(&MethodKind::KeyboardInteractive) {
        let mut response = handle
            .authenticate_keyboard_interactive_start(user, None)
            .await?;
        for _ in 0..8 {
            response = match response {
                KeyboardInteractiveAuthResponse::Success => return Ok(true),
                KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
                KeyboardInteractiveAuthResponse::InfoRequest {
                    name,
                    instructions,
                    prompts,
                } => {
                    let mut answers = Vec::with_capacity(prompts.len());
                    for prompt in prompts {
                        let label = [name.as_str(), instructions.as_str(), prompt.prompt.as_str()]
                            .into_iter()
                            .filter(|part| !part.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n");
                        let Some(answer) = super::interactive::ask(
                            host,
                            super::interactive::PromptKind::Secret {
                                label,
                                echo: prompt.echo,
                            },
                        )
                        .await?
                        else {
                            bail!("{}", crate::t!("ssh.authentication_cancelled"));
                        };
                        answers.push(answer);
                    }
                    handle
                        .authenticate_keyboard_interactive_respond(answers)
                        .await?
                }
            };
        }
        if matches!(response, KeyboardInteractiveAuthResponse::Success) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn expand_tokens(value: &str, host: &str, port: u16, user: &str) -> Result<String> {
    let mut result = String::new();
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '%' {
            result.push(character);
            continue;
        }
        match chars.next() {
            Some('%') => result.push('%'),
            Some('h') => result.push_str(host),
            Some('p') => result.push_str(&port.to_string()),
            Some('r') => result.push_str(user),
            token => bail!(
                "{}",
                crate::t!("ssh.proxy_placeholder", token = format!("{token:?}"))
            ),
        }
    }
    Ok(result)
}
fn jump_destination(value: &str) -> Result<(String, Option<u16>)> {
    ensure!(!value.starts_with('-'), "{}", crate::t!("ssh.jump_invalid"));
    if let Some((host, port)) = value.rsplit_once(':') {
        if !host.contains(':') || host.ends_with(']') {
            return Ok((
                host.to_owned(),
                Some(port.parse().context(crate::t!("ssh.jump_port_invalid"))?),
            ));
        }
    }
    Ok((value.to_owned(), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_profile_uses_only_explicit_host_fields() {
        let host = HostConfig {
            destination: "example.test".into(),
            name: "Example".into(),
            user: "deploy".into(),
            port: Some(2222),
            identity_file: Some(PathBuf::from("test-key")),
            tmux: false,
            socket: None,
        };
        let profile = Profile::direct(&host).unwrap();
        assert_eq!(profile.get("hostname"), "example.test");
        assert_eq!(profile.get("user"), "deploy");
        assert_eq!(profile.get("port"), "2222");
        assert_eq!(
            profile.paths("identityfile"),
            vec![PathBuf::from("test-key")]
        );
        assert!(profile.get("proxyjump").is_empty());
        assert_eq!(profile.get("stricthostkeychecking"), "ask");
    }

    #[test]
    fn direct_known_hosts_path_preserves_spaces() {
        let mut profile = Profile {
            values: BTreeMap::from([(
                "userknownhostsfile".into(),
                vec!["C:/Users/Alice Smith/.ssh/known_hosts".into()],
            )]),
            direct: true,
        };
        assert_eq!(
            profile.known_hosts_files(),
            vec![PathBuf::from("C:/Users/Alice Smith/.ssh/known_hosts")]
        );
        profile.direct = false;
        profile
            .values
            .insert("userknownhostsfile".into(), vec!["first second".into()]);
        assert_eq!(
            profile.known_hosts_files(),
            vec![PathBuf::from("first"), PathBuf::from("second")]
        );
    }
    #[test]
    fn proxy_arguments_keep_spaces_and_percent_literal() {
        assert_eq!(
            expand_tokens("%h:%p/%%/%r", "host", 2222, "user").unwrap(),
            "host:2222/%/user"
        );
        assert_eq!(
            jump_destination("user@jump:2222").unwrap(),
            ("user@jump".into(), Some(2222))
        );
        assert_eq!(
            jump_destination("user@[::1]:2222").unwrap(),
            ("user@[::1]".into(), Some(2222))
        );
        assert!(expand_tokens("%x", "host", 22, "user").is_err());
    }
}
