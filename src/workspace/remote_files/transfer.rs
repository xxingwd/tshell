use super::{
    Operation,
    sftp::{CHUNK, Client},
};
use anyhow::{Context, Result, ensure};
use parking_lot::Mutex;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TransferProgress {
    pub scanning: bool,
    pub transferred: u64,
    pub total: Option<u64>,
    pub current_file: String,
}

#[derive(Clone, Default)]
pub(crate) struct TransferControl {
    cancelled: Arc<AtomicBool>,
    progress: Arc<Mutex<TransferProgress>>,
}

impl TransferControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn snapshot(&self) -> TransferProgress {
        self.progress.lock().clone()
    }

    fn check(&self) -> Result<()> {
        ensure!(!self.is_cancelled(), "{}", crate::t!("transfer.cancelled"));
        Ok(())
    }

    fn scan(&self) {
        self.progress.lock().scanning = true;
    }

    fn begin(&self, total: Option<u64>) {
        let mut progress = self.progress.lock();
        progress.scanning = false;
        progress.total = total;
    }

    fn file(&self, path: &str) {
        self.progress.lock().current_file = path.to_owned();
    }

    fn advance(&self, bytes: usize) {
        let mut progress = self.progress.lock();
        progress.transferred = progress.transferred.saturating_add(bytes as u64);
    }
}

pub(super) async fn download(
    client: &Client,
    source: &str,
    destination: &Path,
    control: &TransferControl,
) -> Result<()> {
    control.scan();
    let total = scan_download(client, source, control, 0).await?;
    control.check()?;
    control.begin(total);
    download_entry(client, source, destination, control, 0).await
}

fn scan_download<'a>(
    client: &'a Client,
    source: &'a str,
    control: &'a TransferControl,
    depth: usize,
) -> Operation<'a, Option<u64>> {
    Box::pin(async move {
        control.check()?;
        ensure!(depth < 64, "{}", crate::t!("file.too_deep"));
        let attrs = client.session.lstat(source).await?.attrs;
        if attrs.file_type().is_file() {
            return Ok(attrs.size);
        }
        ensure!(
            attrs.file_type().is_dir(),
            "{}",
            crate::t!("file.symlink_or_special")
        );
        let entries = client.entries(source, 10001).await?;
        ensure!(
            entries.len() <= 10000,
            "{}",
            crate::t!("file.dir_too_large")
        );
        let mut total = Some(0_u64);
        for entry in entries {
            let child =
                crate::workspace::file_ops::child(Path::new(source), &entry.filename, true)?;
            let child = child.to_string_lossy();
            let size = scan_download(client, &child, control, depth + 1).await?;
            total = total.zip(size).map(|(sum, size)| sum.saturating_add(size));
        }
        Ok(total)
    })
}

fn download_entry<'a>(
    client: &'a Client,
    source: &'a str,
    destination: &'a Path,
    control: &'a TransferControl,
    depth: usize,
) -> Operation<'a, ()> {
    Box::pin(async move {
        control.check()?;
        ensure!(depth < 64, "{}", crate::t!("file.too_deep"));
        let attrs = client.session.lstat(source).await?.attrs;
        if attrs.file_type().is_file() {
            return download_file(client, source, destination, control).await;
        }
        ensure!(
            attrs.file_type().is_dir(),
            "{}",
            crate::t!("file.symlink_or_special")
        );
        let entries = client.entries(source, 10001).await?;
        ensure!(
            entries.len() <= 10000,
            "{}",
            crate::t!("file.dir_too_large")
        );
        let existed = match std::fs::metadata(destination) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_dir(),
                    "{}",
                    crate::t!("file.replace_directory_with_file")
                );
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(destination)
                    .with_context(|| format!("Cannot create {}", destination.display()))?;
                false
            }
            Err(error) => return Err(error.into()),
        };
        let result = async {
            for entry in entries {
                let child = crate::workspace::file_ops::child(destination, &entry.filename, false)?;
                let remote = format!("{}/{}", source.trim_end_matches('/'), entry.filename);
                download_entry(client, &remote, &child, control, depth + 1).await?;
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if result.is_err() && !existed {
            let _ = std::fs::remove_dir_all(destination);
        }
        result
    })
}

async fn download_file(
    client: &Client,
    source: &str,
    destination: &Path,
    control: &TransferControl,
) -> Result<()> {
    control.check()?;
    control.file(source);
    let temporary = temporary_path(destination)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("Cannot create {}", temporary.display()))?;
    let handle = match client
        .session
        .open(source, OpenFlags::READ, FileAttributes::empty())
        .await
    {
        Ok(opened) => opened.handle,
        Err(error) => {
            drop(file);
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
    };
    let result = async {
        let mut offset = 0;
        loop {
            control.check()?;
            match client
                .session
                .read(handle.clone(), offset, CHUNK as u32)
                .await
            {
                Ok(data) => {
                    ensure!(!data.data.is_empty(), "Empty SFTP read");
                    file.write_all(&data.data)?;
                    offset += data.data.len() as u64;
                    control.advance(data.data.len());
                }
                Err(russh_sftp::client::error::Error::Status(status))
                    if status.status_code == StatusCode::Eof =>
                {
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
        control.check()?;
        file.sync_all()?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let closed = client.session.close(handle).await;
    drop(file);
    if result.is_err() || closed.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    closed?;
    replace_local_file(&temporary, destination)?;
    Ok(())
}

fn temporary_path(destination: &Path) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .context(crate::t!("file.destination_parent_missing"))?;
    let name = destination
        .file_name()
        .context("Destination has no file name")?
        .to_string_lossy();
    for index in 0..1000 {
        let temporary = parent.join(format!(
            ".{name}.tshell-transfer-{}-{index}",
            std::process::id()
        ));
        if !temporary.exists() && !temporary.with_extension("old").exists() {
            return Ok(temporary);
        }
    }
    anyhow::bail!("{}", crate::t!("file.temporary_transfer_failed"))
}

fn replace_local_file(temporary: &Path, destination: &Path) -> Result<()> {
    if !destination.exists() {
        std::fs::rename(temporary, destination)?;
        return Ok(());
    }
    ensure!(
        destination.is_file(),
        "{}",
        crate::t!("file.replace_directory_with_file")
    );
    let backup = temporary.with_extension("old");
    std::fs::rename(destination, &backup)?;
    if let Err(error) = std::fs::rename(temporary, destination) {
        let _ = std::fs::rename(&backup, destination);
        let _ = std::fs::remove_file(temporary);
        return Err(error.into());
    }
    let _ = std::fs::remove_file(backup);
    Ok(())
}

pub(super) async fn upload(
    client: &Client,
    sources: &[PathBuf],
    target: &str,
    control: &TransferControl,
) -> Result<()> {
    control.scan();
    let mut total = 0_u64;
    for source in sources {
        total = total.saturating_add(scan_upload(source, control, 0)?);
    }
    control.check()?;
    control.begin(Some(total));
    ensure!(
        client
            .session
            .stat(target)
            .await?
            .attrs
            .file_type()
            .is_dir(),
        "Not a directory"
    );
    for source in sources {
        control.check()?;
        let name = source
            .file_name()
            .context("Source has no file name")?
            .to_string_lossy();
        ensure!(
            !name.contains('/') && !name.contains('\\'),
            "Invalid file name"
        );
        upload_entry(
            client,
            source,
            &format!("{}/{}", target.trim_end_matches('/'), name),
            control,
            0,
        )
        .await?;
    }
    Ok(())
}

fn scan_upload(source: &Path, control: &TransferControl, depth: usize) -> Result<u64> {
    control.check()?;
    ensure!(depth < 64, "{}", crate::t!("file.too_deep"));
    let metadata = std::fs::symlink_metadata(source)?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "{}",
        crate::t!("file.symlink_unsupported")
    );
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    ensure!(metadata.is_dir(), "{}", crate::t!("file.only_regular"));
    let mut total = 0_u64;
    for entry in std::fs::read_dir(source)? {
        total = total.saturating_add(scan_upload(&entry?.path(), control, depth + 1)?);
    }
    Ok(total)
}

fn upload_entry<'a>(
    client: &'a Client,
    source: &'a Path,
    target: &'a str,
    control: &'a TransferControl,
    depth: usize,
) -> Operation<'a, ()> {
    Box::pin(async move {
        control.check()?;
        ensure!(depth < 64, "Directory is too deep");
        let metadata = std::fs::symlink_metadata(source)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "Symbolic links cannot be uploaded"
        );
        if metadata.is_dir() {
            let existed = match client.session.lstat(target).await {
                Ok(attrs) => {
                    ensure!(
                        attrs.attrs.file_type().is_dir(),
                        "{}",
                        crate::t!("file.replace_file_with_directory")
                    );
                    true
                }
                Err(russh_sftp::client::error::Error::Status(status))
                    if status.status_code == StatusCode::NoSuchFile =>
                {
                    client
                        .session
                        .mkdir(target, FileAttributes::empty())
                        .await?;
                    false
                }
                Err(error) => return Err(error.into()),
            };
            let result = async {
                for entry in std::fs::read_dir(source)? {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    ensure!(
                        !name.contains('/') && !name.contains('\\'),
                        "Invalid file name"
                    );
                    let child = format!("{}/{}", target.trim_end_matches('/'), name);
                    upload_entry(client, &entry.path(), &child, control, depth + 1).await?;
                }
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if result.is_err() && !existed {
                let _ = super::operations::remove(client, target, depth).await;
            }
            return result;
        }
        ensure!(metadata.is_file(), "Not a regular file");
        control.file(&source.to_string_lossy());
        let parent = target
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or(".");
        let temporary = format!(
            "{parent}/.tshell-transfer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        );
        let handle = client
            .session
            .open(
                temporary.clone(),
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                FileAttributes::empty(),
            )
            .await?
            .handle;
        let result = async {
            let mut file = std::fs::File::open(source)?;
            let mut offset = 0;
            let mut buffer = [0; CHUNK];
            loop {
                control.check()?;
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                client
                    .session
                    .write(handle.clone(), offset, buffer[..count].to_vec())
                    .await?;
                offset += count as u64;
                control.advance(count);
            }
            control.check()?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let closed = client.session.close(handle).await;
        if result.is_err() || closed.is_err() {
            let _ = client.session.remove(&temporary).await;
        }
        result?;
        closed?;
        if let Err(error) = client.replace(&temporary, target).await {
            let _ = client.session.remove(&temporary).await;
            return Err(error);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_control_tracks_progress_and_cancellation() {
        let control = TransferControl::new();
        control.scan();
        assert!(control.snapshot().scanning);
        control.begin(Some(1024));
        control.file("remote/file.txt");
        control.advance(128);
        let progress = control.snapshot();
        assert!(!progress.scanning);
        assert_eq!(progress.total, Some(1024));
        assert_eq!(progress.transferred, 128);
        assert_eq!(progress.current_file, "remote/file.txt");
        control.cancel();
        assert!(control.is_cancelled());
        assert!(control.check().is_err());
    }
}
