use super::remote_files::Session;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub(super) fn resolve(input: &str, base: &Path) -> PathBuf {
    let path = if input == "~" {
        dirs::home_dir().unwrap_or_else(|| base.to_owned())
    } else if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        dirs::home_dir()
            .unwrap_or_else(|| base.to_owned())
            .join(rest)
    } else {
        PathBuf::from(input)
    };
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

pub(super) fn complete(input: &str, base: &Path, remote: Option<&Session>) -> Result<Vec<String>> {
    if let Some(remote) = remote {
        return super::remote_files::complete(remote, input);
    }
    let path = resolve(input, base);
    let (parent, prefix) = if path.is_dir() {
        (path.as_path(), String::new())
    } else {
        (
            path.parent().context(crate::t!("dir.invalid_path"))?,
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        )
    };
    let matches = |name: &str| {
        if cfg!(windows) {
            name.to_lowercase().starts_with(&prefix.to_lowercase())
        } else {
            name.starts_with(&prefix)
        }
    };
    let mut paths: Vec<_> = std::fs::read_dir(parent)?
        .filter_map(Result::ok)
        .filter(|entry| matches(&entry.file_name().to_string_lossy()) && entry.path().is_dir())
        .map(|entry| format!("{}{}", entry.path().display(), std::path::MAIN_SEPARATOR))
        .collect();
    paths.sort();
    paths.truncate(8);
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_filters_files_and_resolves_relative_prefixes() {
        let base = std::env::temp_dir().join(format!("tshell-complete-{}", std::process::id()));
        std::fs::create_dir_all(base.join("project space")).unwrap();
        std::fs::write(base.join("project.txt"), "").unwrap();
        let candidates = complete("pro", &base, None).unwrap();
        assert_eq!(
            candidates,
            vec![format!(
                "{}{}",
                base.join("project space").display(),
                std::path::MAIN_SEPARATOR
            )]
        );
        std::fs::remove_dir_all(base).unwrap();
    }
}
