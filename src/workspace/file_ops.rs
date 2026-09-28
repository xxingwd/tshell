use super::remote_files;
use anyhow::{Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub(super) enum Operation {
    Create { path: PathBuf, directory: bool },
    Rename { from: PathBuf, to: PathBuf },
    Copy { from: PathBuf, to: PathBuf },
    Delete(PathBuf),
}

pub(super) fn child(parent: &Path, name: &str, remote: bool) -> Result<PathBuf> {
    ensure!(
        !name.trim().is_empty()
            && name != "."
            && name != ".."
            && !name.contains(['/', '\\'])
            && !name.chars().any(char::is_control),
        "{}",
        crate::t!("file.name_required")
    );
    if !remote && cfg!(windows) {
        ensure!(
            !name.contains(['<', '>', ':', '"', '|', '?', '*']) && !name.ends_with(['.', ' ']),
            "{}",
            crate::t!("file.name_charset")
        );
    }
    Ok(if remote {
        PathBuf::from(format!(
            "{}/{}",
            parent.to_string_lossy().trim_end_matches('/'),
            name
        ))
    } else {
        parent.join(name)
    })
}

pub(super) fn execute(host: Option<&remote_files::Session>, operation: &Operation) -> Result<()> {
    if let Some(host) = host {
        return remote_files::operate(host, operation.clone());
    }
    match operation {
        Operation::Create { path, directory } => {
            if *directory {
                fs::create_dir(path)?;
            } else {
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?;
            }
        }
        Operation::Rename { from, to } => {
            match fs::symlink_metadata(to) {
                Ok(_) => anyhow::bail!("{}", crate::t!("file.exists")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            ensure!(!to.starts_with(from), "{}", crate::t!("file.into_self"));
            fs::rename(from, to)?;
        }
        Operation::Copy { from, to } => {
            ensure!(
                !to.starts_with(from),
                "{}",
                crate::t!("file.copy_into_self")
            );
            copy(from, to, 0)?;
        }
        Operation::Delete(path) => {
            let meta = fs::symlink_metadata(path)?;
            if meta.is_dir() && !meta.file_type().is_symlink() {
                fs::remove_dir_all(path)?;
            } else {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}

fn copy(from: &Path, to: &Path, depth: usize) -> Result<()> {
    ensure!(depth < 64, "{}", crate::t!("file.too_deep"));
    let meta = fs::symlink_metadata(from)?;
    ensure!(
        !meta.file_type().is_symlink(),
        "{}",
        crate::t!("file.symlink_unsupported")
    );
    if meta.is_dir() {
        fs::create_dir(to)?;
        let result = (|| {
            for entry in fs::read_dir(from)? {
                let entry = entry?;
                copy(&entry.path(), &to.join(entry.file_name()), depth + 1)?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(to);
        }
        result
    } else {
        ensure!(meta.is_file(), "{}", crate::t!("file.only_regular"));
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        let result =
            fs::File::open(from).and_then(|mut input| std::io::copy(&mut input, &mut output));
        drop(output);
        if let Err(error) = result {
            let _ = fs::remove_file(to);
            return Err(error.into());
        }
        fs::set_permissions(to, meta.permissions())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operations_preserve_existing_files_and_copy_binary_trees() {
        let root = std::env::temp_dir().join(format!("tshell-file-ops-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source");
        execute(
            None,
            &Operation::Create {
                path: source.clone(),
                directory: true,
            },
        )
        .unwrap();
        fs::write(source.join("binary"), [0, 255, 128, 13]).unwrap();
        let copy = root.join("copy");
        execute(
            None,
            &Operation::Copy {
                from: source.clone(),
                to: copy.clone(),
            },
        )
        .unwrap();
        assert_eq!(fs::read(copy.join("binary")).unwrap(), [0, 255, 128, 13]);
        assert!(
            execute(
                None,
                &Operation::Rename {
                    from: source.clone(),
                    to: copy.clone()
                }
            )
            .is_err()
        );
        assert!(
            execute(
                None,
                &Operation::Copy {
                    from: source.clone(),
                    to: source.join("nested")
                }
            )
            .is_err()
        );
        assert!(child(&root, "../escape", false).is_err());
        execute(None, &Operation::Delete(copy)).unwrap();
        execute(None, &Operation::Delete(root)).unwrap();
    }
}
