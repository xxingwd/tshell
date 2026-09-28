use super::{Operation, sftp::Client};
use crate::workspace::file_ops;
use anyhow::{Result, ensure};
use russh_sftp::{
    client::error::Error,
    protocol::{FileAttributes, OpenFlags, StatusCode},
};

pub(super) async fn execute(client: &Client, operation: &file_ops::Operation) -> Result<()> {
    match operation {
        file_ops::Operation::Create { path, directory } => {
            let path = path.to_string_lossy();
            if *directory {
                client
                    .session
                    .mkdir(path.as_ref(), FileAttributes::empty())
                    .await?;
            } else {
                let handle = client
                    .session
                    .open(
                        path.as_ref(),
                        OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                        FileAttributes::empty(),
                    )
                    .await?
                    .handle;
                client.session.close(handle).await?;
            }
        }
        file_ops::Operation::Rename { from, to } => {
            let (from, to) = (from.to_string_lossy(), to.to_string_lossy());
            ensure!(
                !to.starts_with(&format!("{}/", from.trim_end_matches('/'))),
                "{}",
                crate::t!("file.into_self")
            );
            // SFTP v3 rename refuses an existing target; do not use posix-rename (which overwrites).
            client.session.rename(from.as_ref(), to.as_ref()).await?;
        }
        file_ops::Operation::Copy { from, to } => {
            let (from, to) = (from.to_string_lossy(), to.to_string_lossy());
            ensure!(
                from != to && !to.starts_with(&format!("{}/", from.trim_end_matches('/'))),
                "{}",
                crate::t!("file.copy_into_self")
            );
            copy(client, &from, &to, 0).await?;
        }
        file_ops::Operation::Delete(path) => remove(client, &path.to_string_lossy(), 0).await?,
    }
    Ok(())
}

pub(super) fn remove<'a>(client: &'a Client, path: &'a str, depth: usize) -> Operation<'a, ()> {
    Box::pin(async move {
        ensure!(depth < 64, "{}", crate::t!("file.too_deep"));
        if client.session.lstat(path).await?.attrs.file_type().is_dir() {
            // Re-read after each batch; unlike the display tree, deletion must never truncate.
            loop {
                let entries = client.entries(path, 256).await?;
                if entries.is_empty() {
                    break;
                }
                for entry in entries {
                    ensure!(
                        !entry.filename.contains('/') && !entry.filename.contains('\0'),
                        "{}",
                        crate::t!("file.invalid_entry")
                    );
                    remove(
                        client,
                        &format!("{}/{}", path.trim_end_matches('/'), entry.filename),
                        depth + 1,
                    )
                    .await?;
                }
            }
            client.session.rmdir(path).await?;
        } else {
            client.session.remove(path).await?;
        }
        Ok(())
    })
}

fn copy<'a>(client: &'a Client, from: &'a str, to: &'a str, depth: usize) -> Operation<'a, ()> {
    Box::pin(async move {
        ensure!(depth < 64, "{}", crate::t!("file.too_deep"));
        let attrs = client.session.lstat(from).await?.attrs;
        ensure!(
            attrs.file_type().is_dir() || attrs.file_type().is_file(),
            "{}",
            crate::t!("file.symlink_or_special")
        );
        if attrs.file_type().is_dir() {
            let entries = client.entries(from, 10001).await?;
            ensure!(
                entries.len() <= 10000,
                "{}",
                crate::t!("file.dir_too_large")
            );
            client.session.mkdir(to, FileAttributes::empty()).await?;
            let result: Result<()> = async {
                for entry in entries {
                    ensure!(
                        !entry.filename.contains('/') && !entry.filename.contains('\0'),
                        "{}",
                        crate::t!("file.invalid_entry")
                    );
                    copy(
                        client,
                        &format!("{}/{}", from.trim_end_matches('/'), entry.filename),
                        &format!("{}/{}", to.trim_end_matches('/'), entry.filename),
                        depth + 1,
                    )
                    .await?;
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                let _ = remove(client, to, depth).await;
            }
            return result;
        }
        let input = client
            .session
            .open(from, OpenFlags::READ, FileAttributes::empty())
            .await?
            .handle;
        let output = client
            .session
            .open(
                to,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                FileAttributes::empty(),
            )
            .await;
        let output = match output {
            Ok(output) => output.handle,
            Err(error) => {
                let _ = client.session.close(input).await;
                return Err(error.into());
            }
        };
        let result: Result<()> = async {
            let mut offset = 0;
            loop {
                match client.session.read(input.clone(), offset, 32768).await {
                    Ok(data) => {
                        ensure!(!data.data.is_empty(), "{}", crate::t!("file.empty_data"));
                        let length = data.data.len() as u64;
                        client
                            .session
                            .write(output.clone(), offset, data.data)
                            .await?;
                        offset += length;
                    }
                    Err(Error::Status(status)) if status.status_code == StatusCode::Eof => break,
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(())
        }
        .await;
        let input_closed = client.session.close(input).await;
        let output_closed = client.session.close(output).await;
        if result.is_err() {
            let _ = client.session.remove(to).await;
        }
        result?;
        input_closed?;
        output_closed?;
        Ok(())
    })
}
