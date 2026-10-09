use std::path::{Path, PathBuf};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PosixPath(String);
impl PosixPath {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn name(&self) -> &str {
        self.0
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
    }
    pub fn parent(&self) -> Option<Self> {
        let value = self.0.trim_end_matches('/');
        let (parent, _) = value.rsplit_once('/')?;
        Some(Self(if parent.is_empty() {
            "/".into()
        } else {
            parent.into()
        }))
    }
    pub fn join(&self, child: &str) -> Self {
        if child.starts_with('/') {
            Self(child.into())
        } else {
            Self(format!("{}/{}", self.0.trim_end_matches('/'), child))
        }
    }
    pub fn relative_to<'a>(&'a self, root: &Self) -> Option<&'a str> {
        let root = root.0.trim_end_matches('/');
        if self.0 == root {
            return Some("");
        }
        self.0.strip_prefix(root)?.strip_prefix('/')
    }
    pub fn is_within(&self, root: &Self) -> bool {
        self.relative_to(root).is_some()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostPath {
    Local(PathBuf),
    Remote(PosixPath),
}
impl HostPath {
    pub fn for_host(path: &Path, remote: bool) -> Self {
        if remote {
            Self::Remote(PosixPath::new(path.to_string_lossy().into_owned()))
        } else {
            Self::Local(path.to_owned())
        }
    }
    pub fn parent(&self) -> Option<Self> {
        match self {
            Self::Local(path) => path.parent().map(|path| Self::Local(path.into())),
            Self::Remote(path) => path.parent().map(Self::Remote),
        }
    }
    pub fn name(&self) -> String {
        match self {
            Self::Local(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Self::Remote(path) => path.name().to_owned(),
        }
    }
    pub fn join(&self, child: &str) -> Self {
        match self {
            Self::Local(path) => Self::Local(path.join(child)),
            Self::Remote(path) => Self::Remote(path.join(child)),
        }
    }
    pub fn as_path_buf(&self) -> PathBuf {
        match self {
            Self::Local(path) => path.clone(),
            Self::Remote(path) => PathBuf::from(path.as_str()),
        }
    }
    pub fn relative_to(&self, root: &Self) -> Option<String> {
        match (self, root) {
            (Self::Local(path), Self::Local(root)) => path
                .strip_prefix(root)
                .ok()
                .map(|path| path.to_string_lossy().into_owned()),
            (Self::Remote(path), Self::Remote(root)) => path.relative_to(root).map(str::to_owned),
            _ => None,
        }
    }
    pub fn is_within(&self, root: &Self) -> bool {
        self.relative_to(root).is_some()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_backslashes_and_drive_like_names_are_literal() {
        let root = PosixPath::new("/srv/C:project");
        let file = root.join(r"folder\name.rs");
        assert_eq!(file.name(), r"folder\name.rs");
        assert_eq!(file.parent(), Some(root.clone()));
        assert_eq!(file.relative_to(&root), Some(r"folder\name.rs"));
        assert!(!PosixPath::new("/srv/C:project-other").is_within(&root));
    }
}
