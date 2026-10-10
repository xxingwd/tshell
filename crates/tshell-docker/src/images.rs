use crate::{actions::Action, resources::format_bytes};
use anyhow::{Result, ensure};
use bollard::{Docker, query_parameters::ImportImageOptionsBuilder};
use bytes::Bytes;
use futures_util::StreamExt;
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) async fn archive(
    docker: &Docker,
    action: Action,
    image: Option<&str>,
    path: &str,
    progress: impl Fn(String),
) -> Result<String> {
    ensure!(
        !path.is_empty(),
        crate::t!("docker.name_required").into_owned()
    );
    if matches!(action, Action::ImportImage) {
        ensure!(
            tokio::fs::metadata(path).await?.is_file(),
            crate::t!("docker.regular_file_required").into_owned()
        );
        let file = tokio::fs::File::open(path).await?;
        let stream = futures_util::stream::try_unfold(file, |mut file| async move {
            let mut chunk = vec![0; 64 * 1024];
            let count = file.read(&mut chunk).await?;
            if count == 0 {
                Ok::<_, std::io::Error>(None)
            } else {
                chunk.truncate(count);
                Ok(Some((Bytes::from(chunk), file)))
            }
        });
        let mut stream = docker.import_image_stream(
            ImportImageOptionsBuilder::default().quiet(false).build(),
            stream,
            None,
        );
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            if let Some(text) = entry.stream {
                progress(text);
            }
        }
    } else {
        let image = image
            .ok_or_else(|| anyhow::anyhow!(crate::t!("docker.select_resource").into_owned()))?;
        match tokio::fs::symlink_metadata(path).await {
            Ok(_) => anyhow::bail!(crate::t!("docker.file_exists").into_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let parent = Path::new(path)
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let temporary = parent.join(format!(".tshell-image-{nonce}.tar"));
        let mut options = tokio::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).await?;
        let result = async {
            let mut stream = docker.export_image(image);
            let mut total = 0u64;
            let mut reported = 0;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                file.write_all(&chunk).await?;
                total += chunk.len() as u64;
                if total.saturating_sub(reported) >= 1024 * 1024 {
                    progress(format_bytes(total));
                    reported = total;
                }
            }
            file.sync_all().await?;
            drop(file);
            tokio::fs::hard_link(&temporary, path).await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let _ = tokio::fs::remove_file(&temporary).await;
        result?;
    }
    Ok(crate::t!("docker.operation_complete").into_owned())
}
