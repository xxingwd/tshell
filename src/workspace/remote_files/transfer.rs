use super::{
    Operation,
    sftp::{CHUNK, Client},
};
use anyhow::{Context, Result, ensure};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(super) async fn download(client: &Client, source: &str, destination: &Path) -> Result<()> {
    download_entry(client, source, destination, 0).await
}

fn download_entry<'a>(
    client: &'a Client,
    source: &'a str,
    destination: &'a Path,
    depth: usize,
) -> Operation<'a, ()> {
    Box::pin(async move {
        ensure!(depth < 64, "{}", crate::t!("file.too_deep"));
        let attrs = client.session.lstat(source).await?.attrs;
        if attrs.file_type().is_file() {
            return download_file(client, source, destination).await;
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
        std::fs::create_dir(destination)
            .with_context(|| format!("Cannot create {}", destination.display()))?;
        let result = async {
            for entry in entries {
                let child = crate::workspace::file_ops::child(destination, &entry.filename, false)?;
                let remote = format!("{}/{}", source.trim_end_matches('/'), entry.filename);
                download_entry(client, &remote, &child, depth + 1).await?;
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if result.is_err() {
            let _ = std::fs::remove_dir_all(destination);
        }
        result
    })
}

async fn download_file(client: &Client, source: &str, destination: &Path) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("Cannot create {}", destination.display()))?;
    let handle = match client
        .session
        .open(source, OpenFlags::READ, FileAttributes::empty())
        .await
    {
        Ok(opened) => opened.handle,
        Err(error) => {
            drop(file);
            let _ = std::fs::remove_file(destination);
            return Err(error.into());
        }
    };
    let result = async {
        let mut offset = 0;
        loop {
            match client
                .session
                .read(handle.clone(), offset, CHUNK as u32)
                .await
            {
                Ok(data) => {
                    ensure!(!data.data.is_empty(), "Empty SFTP read");
                    file.write_all(&data.data)?;
                    offset += data.data.len() as u64;
                }
                Err(russh_sftp::client::error::Error::Status(status))
                    if status.status_code == StatusCode::Eof =>
                {
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
        file.sync_all()?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let closed = client.session.close(handle).await;
    drop(file);
    if result.is_err() || closed.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    result?;
    closed?;
    Ok(())
}

pub(super) async fn upload(client: &Client, sources: &[PathBuf], target: &str) -> Result<()> {
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
            0,
        )
        .await?;
    }
    Ok(())
}

fn upload_entry<'a>(
    client: &'a Client,
    source: &'a Path,
    target: &'a str,
    depth: usize,
) -> Operation<'a, ()> {
    Box::pin(async move {
        ensure!(depth < 64, "Directory is too deep");
        let metadata = std::fs::symlink_metadata(source)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "Symbolic links cannot be uploaded"
        );
        if metadata.is_dir() {
            client
                .session
                .mkdir(target, FileAttributes::empty())
                .await?;
            let result = async {
                for entry in std::fs::read_dir(source)? {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    ensure!(
                        !name.contains('/') && !name.contains('\\'),
                        "Invalid file name"
                    );
                    let child = format!("{}/{}", target.trim_end_matches('/'), name);
                    upload_entry(client, &entry.path(), &child, depth + 1).await?;
                }
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if result.is_err() {
                let _ = super::operations::remove(client, target, depth).await;
            }
            return result;
        }
        ensure!(metadata.is_file(), "Not a regular file");
        let handle = client
            .session
            .open(
                target,
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
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                client
                    .session
                    .write(handle.clone(), offset, buffer[..count].to_vec())
                    .await?;
                offset += count as u64;
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let closed = client.session.close(handle).await;
        if result.is_err() || closed.is_err() {
            let _ = client.session.remove(target).await;
        }
        result?;
        closed?;
        Ok(())
    })
}
