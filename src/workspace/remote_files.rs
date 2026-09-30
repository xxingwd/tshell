use crate::tmux_client::HostConfig;
use anyhow::{Context, Result, ensure};
use std::{future::Future, path::Path, pin::Pin, time::Duration};
mod operations;
mod pool;
pub(crate) use pool::Session;
mod sftp;
mod transfer;
pub(super) use transfer::{TransferControl, TransferProgress};

pub(super) use super::workbench::Node;

type Operation<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

fn run<T: Send + 'static>(
    host: &Session,
    operation: impl for<'a> FnOnce(&'a sftp::Client) -> Operation<'a, T> + Send + 'static,
) -> Result<T> {
    let host = host.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    crate::ssh_pool::runtime().spawn(async move {
        let _ = sender.send(pool::run(host, operation).await);
    });
    receiver.recv().context(crate::t!("sftp.worker_stopped"))?
}

fn run_transfer<T: Send + 'static>(
    host: &Session,
    operation: impl for<'a> FnOnce(&'a sftp::Client) -> Operation<'a, T> + Send + 'static,
) -> Result<T> {
    let host = host.host.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    crate::ssh_pool::runtime().spawn(async move {
        let result = async {
            let client = sftp::connect(&host).await?;
            operation(&client).await
        };
        let _ = sender.send(result.await);
    });
    receiver.recv().context(crate::t!("sftp.worker_stopped"))?
}

pub(super) fn tree(host: &Session, path: &Path) -> Result<Node> {
    tree_expanded(host, path, Default::default())
}

pub(super) fn tree_expanded(
    host: &Session,
    path: &Path,
    expanded: std::collections::BTreeSet<String>,
) -> Result<Node> {
    let path = path.to_string_lossy().into_owned();
    run(host, move |client| {
        Box::pin(async move {
            let path = client.resolve(&path).await?;
            ensure!(
                client
                    .session
                    .stat(path.clone())
                    .await?
                    .attrs
                    .file_type()
                    .is_dir(),
                "{}",
                crate::t!("sftp.workspace_not_directory")
            );
            build_tree(client, path, true, &expanded, &mut 1200).await
        })
    })
}

async fn build_tree(
    client: &sftp::Client,
    path: String,
    root: bool,
    expanded: &std::collections::BTreeSet<String>,
    budget: &mut usize,
) -> Result<Node> {
    let mut node = Node {
        path: path.clone(),
        directory: true,
        children: Vec::new(),
        loaded: root || expanded.contains(&path),
    };
    if !node.loaded || *budget == 0 {
        return Ok(node);
    }
    let mut entries = client.entries(&node.path, *budget).await?;
    entries.sort_by_key(|entry| {
        (
            !entry.attrs.file_type().is_dir(),
            entry.filename.to_lowercase(),
        )
    });
    for entry in entries {
        if *budget == 0 {
            break;
        }
        let directory = entry.attrs.file_type().is_dir();
        if (!directory && !entry.attrs.file_type().is_file())
            || entry.filename.contains('/')
            || entry.filename.contains('\0')
            || (directory
                && [".git", "artifacts", "node_modules", "target"]
                    .contains(&entry.filename.as_str()))
        {
            continue;
        }
        *budget -= 1;
        let path = format!("{}/{}", node.path.trim_end_matches('/'), entry.filename);
        let child = if directory {
            match Box::pin(build_tree(client, path.clone(), false, expanded, budget)).await {
                Ok(child) => child,
                Err(error) if sftp::permission_denied(&error) => Node {
                    path,
                    directory,
                    children: Vec::new(),
                    loaded: true,
                },
                Err(error) => return Err(error),
            }
        } else {
            Node {
                path,
                directory,
                children: Vec::new(),
                loaded: true,
            }
        };
        node.children.push(child);
    }
    Ok(node)
}

pub(super) fn read(host: &Session, path: &Path) -> Result<String> {
    let path = path.to_string_lossy().into_owned();
    run(host, move |client| {
        Box::pin(async move { client.read(&path).await })
    })
}

pub(super) fn read_bytes(host: &Session, path: &Path) -> Result<Vec<u8>> {
    let path = path.to_string_lossy().into_owned();
    run(host, move |client| {
        Box::pin(async move { client.read_bytes(&path).await })
    })
}

pub(super) fn download(host: &Session, source: &Path, destination: &Path) -> Result<()> {
    download_with_control(host, source, destination, TransferControl::new())
}

pub(super) fn download_with_control(
    host: &Session,
    source: &Path,
    destination: &Path,
    control: TransferControl,
) -> Result<()> {
    let source = source.to_string_lossy().into_owned();
    let destination = destination.to_owned();
    run_transfer(host, move |client| {
        Box::pin(async move { transfer::download(client, &source, &destination, &control).await })
    })
}

pub(super) fn upload(
    host: &Session,
    sources: Vec<std::path::PathBuf>,
    target: &Path,
) -> Result<()> {
    upload_with_control(host, sources, target, TransferControl::new())
}

pub(super) fn upload_with_control(
    host: &Session,
    sources: Vec<std::path::PathBuf>,
    target: &Path,
    control: TransferControl,
) -> Result<()> {
    let target = target.to_string_lossy().into_owned();
    run_transfer(host, move |client| {
        Box::pin(async move { transfer::upload(client, &sources, &target, &control).await })
    })
}

pub(super) fn write(host: &Session, path: &Path, text: &str, expected: &str) -> Result<()> {
    let path = path.to_string_lossy().into_owned();
    let text = text.to_owned();
    let expected = expected.to_owned();
    run(host, move |client| {
        Box::pin(async move { client.write(&path, &text, &expected).await })
    })
}

#[cfg(test)]
mod tests;

pub(super) fn directory(host: &Session, path: &str) -> Result<String> {
    let path = path.to_owned();
    run(host, move |client| {
        Box::pin(async move {
            let path = client.resolve(&path).await?;
            ensure!(
                client
                    .session
                    .stat(path.clone())
                    .await?
                    .attrs
                    .file_type()
                    .is_dir(),
                "{}",
                crate::t!("sftp.not_directory")
            );
            Ok(path)
        })
    })
}

pub(super) fn complete(host: &Session, input: &str) -> Result<Vec<String>> {
    let input = input.to_owned();
    run(host, move |client| {
        Box::pin(async move {
            let input = if input.is_empty() { "~" } else { &input };
            let not_found = |error: &anyhow::Error| {
                error.downcast_ref::<russh_sftp::client::error::Error>().is_some_and(|e| matches!(e, russh_sftp::client::error::Error::Status(s) if s.status_code == russh_sftp::protocol::StatusCode::NoSuchFile))
            };
            let directory = async {
                let path = client.resolve(input).await?;
                let is_dir = client
                    .session
                    .stat(path.clone())
                    .await?
                    .attrs
                    .file_type()
                    .is_dir();
                Ok::<_, anyhow::Error>(is_dir.then_some(path))
            }
            .await;
            let (parent, prefix) = match directory {
                Ok(Some(path)) => (path, String::new()),
                Ok(None) => {
                    let (parent, prefix) = input.rsplit_once('/').unwrap_or(("~", input));
                    (
                        client
                            .resolve(if parent.is_empty() { "/" } else { parent })
                            .await?,
                        prefix.to_owned(),
                    )
                }
                Err(error) if not_found(&error) => {
                    let (parent, prefix) = input.rsplit_once('/').unwrap_or(("~", input));
                    (
                        client
                            .resolve(if parent.is_empty() { "/" } else { parent })
                            .await?,
                        prefix.to_owned(),
                    )
                }
                Err(error) => return Err(error),
            };
            let mut paths = client
                .entries(&parent, 4096)
                .await?
                .into_iter()
                .filter(|e| e.attrs.file_type().is_dir() && e.filename.starts_with(&prefix))
                .map(|e| format!("{}/{}/", parent.trim_end_matches('/'), e.filename))
                .collect::<Vec<_>>();
            paths.sort();
            paths.truncate(8);
            Ok(paths)
        })
    })
}

pub(super) fn operate(host: &Session, operation: super::file_ops::Operation) -> Result<()> {
    run(host, move |client| {
        Box::pin(async move { operations::execute(client, &operation).await })
    })
}
