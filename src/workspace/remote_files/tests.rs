use super::*;
use russh_sftp::protocol::{FileAttributes, OpenFlags};

struct Fixture {
    host: Session,
    root: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let root = self.root.clone();
        let _ = run(&self.host, move |client| {
            Box::pin(async move {
                for entry in client.entries(&root, 100).await? {
                    let path = format!("{root}/{}", entry.filename);
                    if entry.attrs.file_type().is_dir() {
                        client.session.rmdir(path).await?;
                    } else {
                        client.session.remove(path).await?;
                    }
                }
                client.session.rmdir(root).await?;
                Ok(())
            })
        });
    }
}

async fn put(client: &sftp::Client, path: String, data: Vec<u8>) -> Result<()> {
    let handle = client
        .session
        .open(
            path,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
            FileAttributes {
                permissions: Some(0o640),
                ..FileAttributes::empty()
            },
        )
        .await?
        .handle;
    client.session.write(handle.clone(), 0, data).await?;
    client.session.close(handle).await?;
    Ok(())
}

#[test]
#[ignore = "requires TSHELL_SSH_TEST_HOST with SFTP; creates and removes an isolated directory via SFTP only"]
fn remote_file_round_trip_and_conflict() -> Result<()> {
    let host = HostConfig {
        destination: std::env::var("TSHELL_SSH_TEST_HOST")?,
        name: String::new(),
        user: String::new(),
        port: None,
        identity_file: None,
        tmux: false,

        socket: None,
    };
    let host = Session::new(host);
    let root = run(&host, |client| {
        Box::pin(async move {
            let root = format!(
                "{}/.tshell-sftp-test-{}-{}",
                client.resolve("~").await?,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            );
            client
                .session
                .mkdir(
                    root.clone(),
                    FileAttributes {
                        permissions: Some(0o700),
                        ..FileAttributes::empty()
                    },
                )
                .await?;
            Ok(root)
        })
    })?;
    let _fixture = Fixture {
        host: host.clone(),
        root: root.clone(),
    };
    let unusual = format!("{root}/中文 ' $(literal) \\ file.rs");
    let (setup_root, setup_path) = (root.clone(), unusual.clone());
    run(&host, move |client| {
        Box::pin(async move {
            client
                .session
                .mkdir(format!("{setup_root}/empty"), FileAttributes::empty())
                .await?;
            put(client, setup_path, b"hello\r\n".to_vec()).await
        })
    })?;
    {
        use crate::workspace::file_ops::Operation;
        let folder = std::path::PathBuf::from(format!("{root}/ops"));
        let renamed = std::path::PathBuf::from(format!("{root}/renamed"));
        let copied = std::path::PathBuf::from(format!("{root}/copied"));
        operate(
            &host,
            Operation::Create {
                path: folder.clone(),
                directory: true,
            },
        )?;
        let empty = crate::workspace::file_ops::child(&folder, "empty.txt", true)?;
        operate(
            &host,
            Operation::Create {
                path: empty.clone(),
                directory: false,
            },
        )?;
        assert!(
            operate(
                &host,
                Operation::Create {
                    path: empty.clone(),
                    directory: false
                }
            )
            .is_err()
        );
        operate(
            &host,
            Operation::Copy {
                from: Path::new(&unusual).to_owned(),
                to: crate::workspace::file_ops::child(&folder, "copy.txt", true)?,
            },
        )?;
        operate(
            &host,
            Operation::Copy {
                from: folder.clone(),
                to: copied.clone(),
            },
        )?;
        assert_eq!(
            read(
                &host,
                &crate::workspace::file_ops::child(&copied, "copy.txt", true)?
            )?,
            "hello\r\n"
        );
        assert!(
            operate(
                &host,
                Operation::Rename {
                    from: folder.clone(),
                    to: copied.clone()
                }
            )
            .is_err()
        );
        operate(
            &host,
            Operation::Rename {
                from: folder,
                to: renamed.clone(),
            },
        )?;
        operate(&host, Operation::Delete(renamed))?;
        operate(&host, Operation::Delete(copied))?;
    }
    let path = Path::new(&unusual);
    assert_eq!(
        complete(&host, &format!("{root}/em"))?,
        vec![format!("{root}/empty/")]
    );
    assert!(
        complete(&host, "~")?
            .iter()
            .all(|p| p.starts_with('/') && p.ends_with('/'))
    );
    let node = tree(&host, Path::new(&root))?;
    assert!(
        node.children
            .iter()
            .any(|child| child.path == unusual && !child.directory)
    );
    assert!(
        node.children
            .iter()
            .any(|child| child.directory && child.path.ends_with("/empty"))
    );
    assert_eq!(read(&host, path)?, "hello\r\n");
    let local = std::env::temp_dir().join(format!("tshell-transfer-{}", std::process::id()));
    std::fs::create_dir(&local)?;
    let source = local.join("upload.bin");
    std::fs::write(&source, [0, 255, 42])?;
    let folder = local.join("upload-folder");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("nested.txt"), "nested")?;
    upload(&host, vec![source, folder], Path::new(&root))?;
    let uploaded = Path::new(&root).join("upload.bin");
    assert_eq!(read_bytes(&host, &uploaded)?, [0, 255, 42]);
    let source = local.join("upload.bin");
    std::fs::write(&source, [9, 8, 7])?;
    upload(&host, vec![source], Path::new(&root))?;
    assert_eq!(read_bytes(&host, &uploaded)?, [9, 8, 7]);
    assert_eq!(
        read(&host, &Path::new(&root).join("upload-folder/nested.txt"))?,
        "nested"
    );
    let downloaded = local.join("download.bin");
    download(&host, &uploaded, &downloaded)?;
    assert_eq!(std::fs::read(&downloaded)?, [9, 8, 7]);
    std::fs::write(&downloaded, [1, 2, 3])?;
    download(&host, &uploaded, &downloaded)?;
    assert_eq!(std::fs::read(&downloaded)?, [9, 8, 7]);
    let downloaded_folder = local.join("downloaded-folder");
    download(
        &host,
        &Path::new(&root).join("upload-folder"),
        &downloaded_folder,
    )?;
    assert_eq!(
        std::fs::read_to_string(downloaded_folder.join("nested.txt"))?,
        "nested"
    );
    download(
        &host,
        &Path::new(&root).join("upload-folder"),
        &downloaded_folder,
    )?;
    operate(
        &host,
        crate::workspace::file_ops::Operation::Delete(uploaded),
    )?;
    operate(
        &host,
        crate::workspace::file_ops::Operation::Delete(Path::new(&root).join("upload-folder")),
    )?;
    std::fs::remove_dir_all(local)?;
    write(&host, path, "你好\nfn main() {}\n", "hello\r\n")?;
    assert_eq!(read(&host, path)?, "你好\nfn main() {}\n");
    assert!(
        format!(
            "{:#}",
            write(&host, path, "lost update", "hello\r\n").unwrap_err()
        )
        .contains("修改")
    );
    assert_eq!(read(&host, path)?, "你好\nfn main() {}\n");
    let verify_path = unusual.clone();
    let permissions = run(&host, move |client| {
        Box::pin(async move {
            Ok(client
                .session
                .stat(verify_path)
                .await?
                .attrs
                .permissions
                .unwrap()
                & 0o777)
        })
    })?;
    assert_eq!(permissions, 0o640);
    let invalid_path = unusual.clone();
    run(&host, move |client| {
        Box::pin(async move { put(client, invalid_path, vec![255]).await })
    })?;
    assert!(format!("{:#}", read(&host, path).unwrap_err()).contains("UTF-8"));
    let large_path = unusual.clone();
    run(&host, move |client| {
        Box::pin(async move {
            client
                .session
                .setstat(
                    large_path,
                    FileAttributes {
                        size: Some(sftp::MAX_BYTES as u64 + 1),
                        ..FileAttributes::empty()
                    },
                )
                .await?;
            Ok(())
        })
    })?;
    assert!(format!("{:#}", read(&host, path).unwrap_err()).contains("20 MiB"));
    Ok(())
}
