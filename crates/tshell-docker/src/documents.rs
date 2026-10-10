use crate::{
    Target,
    resources::{Resource, host},
};
use anyhow::{Result, ensure};
use gpui_kit::component::input::EditorState;
use gpui_kit::{prelude::*, *};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug)]
pub(crate) struct File {
    pub path: String,
    pub original: Option<String>,
    pub backup: Option<String>,
}
pub(crate) struct Document {
    pub file: File,
    pub editor: Entity<EditorState>,
    initial: String,
}
impl Document {
    pub fn new(
        file: File,
        resource: &Resource,
        window: &mut Window,
        cx: &mut Context<crate::DockerView>,
    ) -> Self {
        let language = if *resource == Resource::Daemon {
            "json"
        } else {
            "yaml"
        };
        let value = file.original.clone().unwrap_or_else(|| {
            if language == "json" {
                "{}\n".into()
            } else {
                String::new()
            }
        });
        let initial = value.clone();
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language(language)
                .default_value(value);
            editor.set_highlighter_factory(tshell_ui::editor::factory(), cx);
            editor
        });
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        Self {
            file,
            editor,
            initial,
        }
    }
    pub fn dirty(&self, cx: &App) -> bool {
        self.editor.read(cx).value().as_ref()
            != self.file.original.as_deref().unwrap_or(&self.initial)
    }
    pub fn discard(&self, window: &mut Window, cx: &mut Context<crate::DockerView>) {
        let value = self.file.original.as_ref().unwrap_or(&self.initial).clone();
        self.editor
            .update(cx, |editor, cx| editor.set_value(value, window, cx));
    }
}
pub(crate) fn read(target: &Target, path: String) -> Result<File> {
    ensure!(
        !path.is_empty(),
        crate::t!("docker.config_source_required").into_owned()
    );
    let original = match host(target) {
        tshell_host::Host::Local => {
            if let Ok(metadata) = fs::symlink_metadata(&path) {
                ensure!(
                    metadata.file_type().is_file() && metadata.len() <= 2 * 1024 * 1024,
                    crate::t!("docker.regular_file_required").into_owned()
                );
            }
            match fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            }
        }
        tshell_host::Host::Ssh(config) => {
            let session = tshell_host::sftp::Session::new(config);
            tshell_host::sftp::read_optional(&session, &PathBuf::from(&path))?
        }
    };
    ensure!(
        original
            .as_ref()
            .is_none_or(|text| text.len() <= 2 * 1024 * 1024),
        crate::t!("host.output_too_large").into_owned()
    );
    Ok(File {
        path,
        original,
        backup: None,
    })
}
pub(crate) fn save(target: &Target, file: &File, text: &str) -> Result<File> {
    ensure!(
        text.len() <= 2 * 1024 * 1024,
        crate::t!("host.output_too_large").into_owned()
    );
    let current = read(target, file.path.clone())?;
    ensure!(
        current.original == file.original,
        crate::t!("docker.file_changed").into_owned()
    );
    let backup = match host(target) {
        tshell_host::Host::Local => {
            let path = PathBuf::from(&file.path);
            let parent = path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            let temporary = parent.join(format!(".tshell-docker-{}-{nonce}", std::process::id()));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut output = options.open(&temporary)?;
            let backup = file
                .original
                .as_ref()
                .map(|_| format!("{}.tshell-{nonce}.bak", file.path));
            let result = (|| {
                use std::io::Write;
                if file.original.is_some() {
                    output.set_permissions(fs::metadata(&path)?.permissions())?;
                }
                output.write_all(text.as_bytes())?;
                output.sync_all()?;
                drop(output);
                ensure!(
                    read(target, file.path.clone())?.original == file.original,
                    crate::t!("docker.file_changed").into_owned()
                );
                if file.original.is_some() {
                    if let Some(backup) = &backup {
                        fs::copy(&path, backup)?;
                    }
                }
                if file.original.is_none() {
                    fs::hard_link(&temporary, &path)?;
                    fs::remove_file(&temporary)?;
                } else {
                    fs::rename(&temporary, &path)?;
                }
                Ok::<_, anyhow::Error>(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            result?;
            backup
        }
        tshell_host::Host::Ssh(config) => {
            let session = tshell_host::sftp::Session::new(config);
            tshell_host::sftp::write_with_backup(
                &session,
                &PathBuf::from(&file.path),
                text,
                file.original.as_deref(),
            )?
        }
    };
    Ok(File {
        path: file.path.clone(),
        original: Some(text.into()),
        backup,
    })
}
pub(crate) fn parent(path: &str, target: &Target) -> Option<String> {
    match &target.endpoint {
        crate::Endpoint::Local { .. } => PathBuf::from(path)
            .parent()
            .map(|path| path.to_string_lossy().into_owned()),
        crate::Endpoint::Ssh(_) => path.rsplit_once('/').map(|(parent, _)| {
            if parent.is_empty() {
                "/".into()
            } else {
                parent.into()
            }
        }),
    }
}
pub(crate) fn ensure_missing(target: &Target, path: &str) -> Result<()> {
    let missing = match host(target) {
        tshell_host::Host::Local => match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(error) => return Err(error.into()),
            Ok(_) => false,
        },
        host => host
            .run(tshell_host::CommandRequest::new(
                "sh",
                ["-c", "test ! -e \"$1\" && test ! -L \"$1\"", "tshell", path],
            ))
            .is_ok(),
    };
    ensure!(missing, crate::t!("docker.file_exists").into_owned());
    Ok(())
}
pub(crate) fn remove_temporary(target: &Target, path: &str) {
    match host(target) {
        tshell_host::Host::Local => {
            let _ = fs::remove_file(path);
        }
        host => {
            let _ = host.run(tshell_host::CommandRequest::new("rm", ["--", path]));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{read, save};
    use crate::Target;
    use std::fs;
    #[test]
    fn saving_a_changed_file_preserves_external_content_and_original_snapshot() {
        let path = std::env::temp_dir().join(format!(
            "tshell-docker-doc-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, "original").unwrap();
        let target = Target::local("test".into(), None);
        let file = read(&target, path.to_string_lossy().into()).unwrap();
        fs::write(&path, "external").unwrap();
        assert!(save(&target, &file, "editor").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "external");
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn saving_and_restoring_keep_each_previous_file_and_new_creation_has_no_backup() {
        let path = std::env::temp_dir().join(format!(
            "tshell-docker-backup-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let target = Target::local("test".into(), None);
        let file = read(&target, path.to_string_lossy().into()).unwrap();
        let first = save(&target, &file, "original").unwrap();
        assert!(first.backup.is_none());
        let second = save(&target, &first, "changed").unwrap();
        let backup = second.backup.as_ref().unwrap();
        assert_eq!(fs::read_to_string(backup).unwrap(), "original");
        let third = save(&target, &second, &fs::read_to_string(backup).unwrap()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        assert_eq!(
            fs::read_to_string(third.backup.as_ref().unwrap()).unwrap(),
            "changed"
        );
        fs::remove_file(path).unwrap();
        fs::remove_file(backup).unwrap();
        fs::remove_file(third.backup.unwrap()).unwrap();
    }
}
