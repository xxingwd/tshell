use crate::SshConfig;
use crate::diagnostic::ResultExt;
use anyhow::{Result, bail, ensure};
use russh_sftp::{
    client::{RawSftpSession, error::Error},
    protocol::{File, FileAttributes, OpenFlags, Packet, StatusCode},
};
use std::time::Duration;
use tokio::time::timeout;

pub(super) const MAX_BYTES: usize = crate::files::MAX_FILE_BYTES as usize;
pub(super) const CHUNK: usize = 32 * 1024;

pub(super) struct Client {
    pub session: RawSftpSession,
    posix_rename: bool,
    fsync: bool,
}

pub(super) async fn connect(host: &SshConfig) -> Result<Client> {
    let mut channel = crate::ssh::open_file(host).await?;
    channel.request_subsystem(true, "sftp").await?;
    crate::ssh::confirmed(&mut channel).await?;
    let session = RawSftpSession::new(channel.into_stream());
    session.set_timeout(10);
    let version = timeout(Duration::from_secs(15), session.init())
        .await
        .with_diagnostic(crate::diagnostic!("sftp.init_timeout"))??;
    ensure!(
        version.version == 3,
        crate::diagnostic!("sftp.version_unsupported", version = version.version)
    );
    Ok(Client {
        posix_rename: version
            .extensions
            .get("posix-rename@openssh.com")
            .is_some_and(|version| version == "1"),
        fsync: version
            .extensions
            .get("fsync@openssh.com")
            .is_some_and(|version| version == "1"),
        session,
    })
}

fn is_status(error: &Error, code: StatusCode) -> bool {
    matches!(error, Error::Status(status) if status.status_code == code)
}

pub(super) fn permission_denied(error: &anyhow::Error) -> bool {
    crate::diagnostic::find_cause::<Error>(error)
        .is_some_and(|error| is_status(error, StatusCode::PermissionDenied))
}

impl Client {
    pub async fn resolve(&self, path: &str) -> Result<String> {
        let path = if path == "~" || path.is_empty() {
            ".".to_owned()
        } else if let Some(rest) = path.strip_prefix("~/") {
            format!("{}/{}", self.canonical(".").await?, rest)
        } else {
            path.to_owned()
        };
        self.canonical(&path).await
    }

    async fn canonical(&self, path: &str) -> Result<String> {
        self.session
            .realpath(path)
            .await?
            .files
            .into_iter()
            .next()
            .map(|file| file.filename)
            .with_diagnostic(crate::diagnostic!("sftp.empty_path"))
    }

    pub async fn entries(&self, path: &str, limit: usize) -> Result<Vec<File>> {
        let handle = self.session.opendir(path).await?.handle;
        let result = async {
            let mut files = Vec::new();
            while files.len() < limit {
                match self.session.readdir(handle.clone()).await {
                    Ok(page) => {
                        ensure!(
                            !page.files.is_empty(),
                            crate::diagnostic!("sftp.empty_page")
                        );
                        files.extend(
                            page.files
                                .into_iter()
                                .filter(|file| file.filename != "." && file.filename != "..")
                                .take(limit - files.len()),
                        );
                    }
                    Err(error) if is_status(&error, StatusCode::Eof) => break,
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(files)
        }
        .await;
        let closed = self.session.close(handle).await;
        let files = result?;
        closed?;
        Ok(files)
    }

    pub async fn read(&self, path: &str) -> Result<String> {
        String::from_utf8(self.read_bytes(path).await?)
            .with_diagnostic(crate::diagnostic!("sftp.not_utf8"))
    }
    pub async fn read_optional(&self, path: &str) -> Result<Option<String>> {
        match self.session.lstat(path).await {
            Ok(metadata) => {
                ensure!(
                    metadata.attrs.file_type().is_file(),
                    crate::diagnostic!("sftp.regular_only")
                );
                self.read(path).await.map(Some)
            }
            Err(error) if is_status(&error, StatusCode::NoSuchFile) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    pub async fn write_with_backup(
        &self,
        path: &str,
        text: &str,
        expected: Option<&str>,
    ) -> Result<Option<String>> {
        ensure!(
            self.read_optional(path).await?.as_deref() == expected,
            crate::diagnostic!("sftp.remote_modified")
        );
        if let Some(expected) = expected {
            let attrs = self.session.lstat(path).await?.attrs;
            ensure!(
                attrs.file_type().is_file(),
                crate::diagnostic!("sftp.regular_only")
            );
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            let backup = format!("{path}.tshell-{nonce}.bak");
            self.create_text(&backup, expected, attrs).await?;
            self.write(path, text, expected).await?;
            Ok(Some(backup))
        } else {
            self.create_text(
                path,
                text,
                FileAttributes {
                    permissions: Some(0o600),
                    ..FileAttributes::empty()
                },
            )
            .await?;
            Ok(None)
        }
    }
    async fn create_text(&self, path: &str, text: &str, attrs: FileAttributes) -> Result<()> {
        ensure!(
            text.len() <= MAX_BYTES,
            crate::diagnostic!("sftp.too_large")
        );
        let parent = path
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or(".");
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let temporary = format!("{parent}/.tshell-new-{}-{nonce}", std::process::id());
        let handle = self
            .session
            .open(
                temporary.clone(),
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                FileAttributes {
                    permissions: Some(0o600),
                    ..FileAttributes::empty()
                },
            )
            .await?
            .handle;
        let written = async {
            for (index, chunk) in text.as_bytes().chunks(CHUNK).enumerate() {
                self.session
                    .write(handle.clone(), (index * CHUNK) as u64, chunk.to_vec())
                    .await?;
            }
            self.session
                .fsetstat(
                    handle.clone(),
                    FileAttributes {
                        permissions: attrs.permissions,
                        uid: attrs.uid,
                        gid: attrs.gid,
                        ..FileAttributes::empty()
                    },
                )
                .await?;
            if self.fsync {
                self.session.fsync(handle.clone()).await?;
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let closed = self.session.close(handle).await;
        let result = async {
            written?;
            closed?;
            // SFTP v3 rename refuses an existing destination, including a concurrent creator.
            self.session.rename(&temporary, path).await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if result.is_err() {
            let _ = self.session.remove(temporary).await;
        }
        result
    }

    pub async fn read_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let attrs = self.session.stat(path).await?.attrs;
        ensure!(
            attrs.file_type().is_file(),
            crate::diagnostic!("sftp.not_regular")
        );
        ensure!(
            attrs.size.unwrap_or(0) <= MAX_BYTES as u64,
            crate::diagnostic!("sftp.too_large")
        );
        let handle = self
            .session
            .open(path, OpenFlags::READ, FileAttributes::empty())
            .await?
            .handle;
        let result = async {
            let mut bytes = Vec::new();
            loop {
                match self
                    .session
                    .read(handle.clone(), bytes.len() as u64, CHUNK as u32)
                    .await
                {
                    Ok(data) => {
                        ensure!(!data.data.is_empty(), crate::diagnostic!("sftp.empty_data"));
                        bytes.extend(data.data);
                        ensure!(
                            bytes.len() <= MAX_BYTES,
                            crate::diagnostic!("sftp.too_large")
                        );
                    }
                    Err(error) if is_status(&error, StatusCode::Eof) => break,
                    Err(error) => return Err(error.into()),
                }
            }
            Ok::<_, anyhow::Error>(bytes)
        }
        .await;
        let closed = self.session.close(handle).await;
        let bytes = result?;
        closed?;
        Ok(bytes)
    }

    pub async fn write(&self, path: &str, text: &str, expected: &str) -> Result<()> {
        ensure!(
            text.len() <= MAX_BYTES,
            crate::diagnostic!("sftp.too_large")
        );
        ensure!(
            self.posix_rename,
            crate::diagnostic!("sftp.no_atomic_rename")
        );
        let original = self.session.lstat(path).await?.attrs;
        ensure!(
            original.file_type().is_file(),
            crate::diagnostic!("sftp.regular_only")
        );
        ensure!(
            self.read(path).await? == expected,
            crate::diagnostic!("sftp.remote_modified")
        );
        let parent = path
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or(".");
        let temporary = format!(
            "{parent}/.tshell-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        );
        let attrs = FileAttributes {
            permissions: Some(0o600),
            ..FileAttributes::empty()
        };
        let handle = self
            .session
            .open(
                temporary.clone(),
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                attrs,
            )
            .await?
            .handle;
        let result = async {
            for (index, data) in text.as_bytes().chunks(CHUNK).enumerate() {
                self.session
                    .write(handle.clone(), (index * CHUNK) as u64, data.to_vec())
                    .await?;
            }
            self.session
                .fsetstat(
                    handle.clone(),
                    FileAttributes {
                        permissions: original.permissions,
                        uid: original.uid,
                        gid: original.gid,
                        ..FileAttributes::empty()
                    },
                )
                .await?;
            if self.fsync {
                self.session.fsync(handle.clone()).await?;
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let closed = self.session.close(handle).await;
        let result = async {
            result?;
            closed?;
            ensure!(
                self.read(path).await? == expected,
                crate::diagnostic!("sftp.save_cancelled")
            );
            let mut data = Vec::new();
            for path in [temporary.as_str(), path] {
                data.extend_from_slice(&(path.len() as u32).to_be_bytes());
                data.extend_from_slice(path.as_bytes());
            }
            match self
                .session
                .extended("posix-rename@openssh.com", data)
                .await?
            {
                Packet::Status(status) if status.status_code == StatusCode::Ok => Ok(()),
                Packet::Status(status) => Err(Error::Status(status).into()),
                _ => bail!(crate::diagnostic!("sftp.rename_invalid")),
            }
        }
        .await;
        if result.is_err() {
            let _ = self.session.remove(temporary).await;
        }
        result
    }

    pub async fn replace(&self, temporary: &str, target: &str) -> Result<()> {
        if self.posix_rename {
            let mut data = Vec::with_capacity(8 + temporary.len() + target.len());
            for path in [temporary, target] {
                data.extend_from_slice(&(path.len() as u32).to_be_bytes());
                data.extend_from_slice(path.as_bytes());
            }
            match self
                .session
                .extended("posix-rename@openssh.com", data)
                .await?
            {
                Packet::Status(status) if status.status_code == StatusCode::Ok => return Ok(()),
                Packet::Status(status) => return Err(Error::Status(status).into()),
                _ => bail!(crate::diagnostic!("sftp.rename_invalid")),
            }
        }
        match self.session.remove(target).await {
            Ok(_) => {}
            Err(error) if is_status(&error, StatusCode::NoSuchFile) => {}
            Err(error) => return Err(error.into()),
        }
        self.session.rename(temporary, target).await?;
        Ok(())
    }
}
