mod install;
pub(crate) mod manifest;

use anyhow::{Context, Result, ensure};
use manifest::{ASSET, FEED, MAX_MANIFEST, Manifest};
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::{OnceLock, mpsc},
    time::Duration,
};

const CURRENT: &str = env!("CARGO_PKG_VERSION");
const REPOSITORY: &str = match option_env!("TSHELL_UPDATE_REPOSITORY") {
    Some(value) => value,
    None => "xxingwd/tshell",
};
static STARTUP_ERROR: OnceLock<String> = OnceLock::new();

#[derive(Clone, Debug)]
pub enum State {
    Disabled,
    Idle,
    Checking,
    Downloading(String),
    Current,
    Ready(String),
    Failed(String),
}

pub struct Updater {
    requests: Option<mpsc::SyncSender<()>>,
    pub state: State,
}

impl Updater {
    pub fn start() -> (Self, async_channel::Receiver<State>) {
        let (events, receiver) = async_channel::unbounded();
        let Some(config) = Config::embedded() else {
            return (
                Self {
                    requests: None,
                    state: State::Disabled,
                },
                receiver,
            );
        };
        let (requests, commands) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("updates".into())
            .spawn(move || {
                let mut delay = Duration::from_secs(15);
                loop {
                    match commands.recv_timeout(delay) {
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        _ => {}
                    }
                    let send = |state| {
                        let _ = events.send_blocking(state);
                    };
                    match config.check_and_stage(&send) {
                        Ok(state) => send(state),
                        Err(error) => send(State::Failed(format!("{error:#}"))),
                    }
                    delay = Duration::from_secs(6 * 60 * 60);
                }
            });
        let state = match worker {
            Ok(_) => STARTUP_ERROR
                .get()
                .cloned()
                .map(State::Failed)
                .unwrap_or(State::Idle),
            Err(error) => State::Failed(error.to_string()),
        };
        (
            Self {
                requests: Some(requests),
                state,
            },
            receiver,
        )
    }

    pub fn check(&mut self) {
        if let Some(requests) = &self.requests {
            if requests.try_send(()).is_ok() {
                self.state = State::Checking;
            }
        }
    }
}

struct Config;

impl Config {
    fn embedded() -> Option<Self> {
        if cfg!(debug_assertions) || !cfg!(target_arch = "x86_64") {
            return None;
        }
        if !valid_repository(REPOSITORY) {
            return None;
        }
        Some(Self)
    }

    fn check_and_stage(&self, send: &impl Fn(State)) -> Result<State> {
        let paths = Paths::new()?;
        let _lock = paths.lock("download.lock")?;
        if paths.pending.exists() {
            match self.pending(&paths) {
                Ok(pending) if pending.newer_than(CURRENT)? => {
                    return Ok(State::Ready(pending.version));
                }
                Ok(_) => {
                    fs::remove_file(&paths.pending)?;
                    fs::remove_file(&paths.binary)?;
                }
                Err(error) => {
                    tracing::warn!("Discarding invalid staged update: {error:#}");
                    // A corrupt download must not permanently prevent future checks.
                    fs::remove_file(&paths.pending)?;
                }
            }
        }
        send(State::Checking);
        let client = Client::builder()
            .https_only(true)
            .user_agent(concat!("TShell/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()?;
        let feed = format!("https://github.com/{REPOSITORY}/releases/latest/download/{FEED}");
        let response = client.get(feed).send()?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(State::Current);
        }
        let mut bytes = Vec::new();
        response
            .error_for_status()?
            .take(MAX_MANIFEST + 1)
            .read_to_end(&mut bytes)?;
        let manifest = manifest::parse(&bytes)?;
        if !manifest.newer_than(CURRENT)? {
            return Ok(State::Current);
        }
        send(State::Downloading(manifest.version.clone()));
        let url = format!(
            "https://github.com/{REPOSITORY}/releases/download/v{}/{ASSET}",
            manifest.version
        );
        let mut response = client
            .get(url)
            .send()?
            .error_for_status()?
            .take(manifest.size + 1);
        let part = paths.directory.join("download.part");
        let mut file = File::create(&part)?;
        let count = std::io::copy(&mut response, &mut file)?;
        ensure!(count == manifest.size, "Incomplete or oversized download");
        file.sync_all()?;
        drop(file);
        manifest.verify_binary(File::open(&part)?)?;
        // The manifest is the commit marker. A crash before its rename leaves no installable update.
        if paths.pending.exists() {
            fs::remove_file(&paths.pending)?;
        }
        if paths.binary.exists() {
            fs::remove_file(&paths.binary)?;
        }
        fs::rename(&part, &paths.binary)?;
        let temporary = paths.directory.join("manifest.part");
        let mut file = File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary, &paths.pending)?;
        Ok(State::Ready(manifest.version))
    }

    fn pending(&self, paths: &Paths) -> Result<Manifest> {
        let mut bytes = Vec::new();
        File::open(&paths.pending)?
            .take(MAX_MANIFEST + 1)
            .read_to_end(&mut bytes)?;
        let manifest = manifest::parse(&bytes)?;
        manifest.verify_binary(File::open(&paths.binary)?)?;
        Ok(manifest)
    }
}

fn valid_repository(repository: &str) -> bool {
    let parts: Vec<_> = repository.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
}

struct Paths {
    directory: PathBuf,
    pending: PathBuf,
    binary: PathBuf,
}

impl Paths {
    fn new() -> Result<Self> {
        let exe = std::env::current_exe()?.canonicalize()?;
        // Scope staged updates and process locks to this installation, including portable copies.
        let identity = hex::encode(Sha256::digest(
            exe.to_string_lossy().to_lowercase().as_bytes(),
        ));
        let directory = dirs::cache_dir()
            .context("No cache directory")?
            .join("tshell")
            .join("updates")
            .join(identity);
        fs::create_dir_all(&directory)?;
        Ok(Self {
            pending: directory.join("pending.json"),
            binary: directory.join("pending.exe"),
            directory,
        })
    }

    fn open_lock(&self, name: &str) -> Result<File> {
        Ok(OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.directory.join(name))?)
    }

    fn lock(&self, name: &str) -> Result<File> {
        let file = self.open_lock(name)?;
        file.try_lock()
            .context("Another application instance is using the update cache")?;
        Ok(file)
    }
}

/// Hold a shared installation lease for the entire GUI lifetime. Startup installation
/// needs an exclusive lease, so another running TShell instance prevents replacement.
pub fn startup() -> Option<File> {
    let config = Config::embedded()?;
    match startup_inner(&config) {
        Ok(file) => Some(file),
        Err(error) => {
            tracing::warn!("Update startup: {error:#}");
            let _ = STARTUP_ERROR.set(format!("{error:#}"));
            None
        }
    }
}

fn startup_inner(config: &Config) -> Result<File> {
    let paths = Paths::new()?;
    let lease = paths.open_lock("application.lock")?;
    if lease.try_lock().is_ok() {
        let install_result = (|| -> Result<bool> {
            let _download = paths.lock("download.lock")?;
            if !paths.pending.exists() {
                return Ok(false);
            }
            let pending = config.pending(&paths)?;
            if !pending.newer_than(CURRENT)? {
                return Ok(false);
            }
            let exe = std::env::current_exe()?;
            install::replace(&exe, &paths.binary, || {
                std::process::Command::new(&exe)
                    .args(std::env::args_os().skip(1))
                    .spawn()
                    .context("Could not start updated TShell")?;
                Ok(())
            })?;
            Ok(true)
        })();
        match install_result {
            Ok(true) => {
                drop(lease);
                std::process::exit(0);
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!("Update install: {error:#}");
                let _ = STARTUP_ERROR.set(format!("{error:#}"));
            }
        }
        lease.unlock()?;
    }
    lease.lock_shared()?;
    Ok(lease)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_repository_cannot_inject_a_url() {
        assert!(valid_repository("xxingwd/tshell"));
        for invalid in [
            "https://github.com/x/y",
            "x/y/z",
            "x/..",
            "x/y?token=1",
            "/y",
        ] {
            assert!(!valid_repository(invalid));
        }
    }

    #[test]
    fn update_shared_lease_blocks_installation() {
        let directory = tempfile::tempdir().unwrap();
        let paths = Paths {
            directory: directory.path().into(),
            pending: PathBuf::new(),
            binary: PathBuf::new(),
        };
        let running = paths.open_lock("application.lock").unwrap();
        running.lock_shared().unwrap();
        assert!(paths.lock("application.lock").is_err());
        drop(running);
        assert!(paths.lock("application.lock").is_ok());
    }
}
