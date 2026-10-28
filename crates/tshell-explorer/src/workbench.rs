use anyhow::{Context, Result, bail};
use gpui_kit::component::tree::TreeItem;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

pub use tshell_host::files::Node;
pub trait NodeView {
    fn item(self, root: bool, remote: bool) -> TreeItem;
}
impl NodeView for Node {
    fn item(self, root: bool, remote: bool) -> TreeItem {
        let label = self
            .path
            .rsplit(if cfg!(windows) && !remote {
                &['/', '\\'][..]
            } else {
                &['/'][..]
            })
            .find(|part| !part.is_empty())
            .unwrap_or("/")
            .to_owned();
        let mut children: Vec<_> = self
            .children
            .into_iter()
            .map(|node| node.item(false, remote))
            .collect();
        if self.directory && !self.loaded && children.is_empty() {
            children.push(TreeItem::new(format!("{}#pending", self.path), "…").disabled(true));
        }
        TreeItem::new(self.path, label)
            .expanded(root)
            .children(children)
    }
}

const MAX_TREE_ENTRIES: usize = 1_200;
const IGNORED_DIRECTORIES: &[&str] = &[".git", "artifacts", "node_modules", "target"];

pub fn file_tree(root: &Path) -> Vec<TreeItem> {
    file_nodes(root, &Default::default())
        .item(true, false)
        .children
}

pub fn file_paths(items: &[TreeItem], root: &Path) -> Vec<String> {
    fn collect(items: &[TreeItem], root: &Path, paths: &mut Vec<String>) {
        for item in items {
            let path = PathBuf::from(item.id.as_ref());
            if !item.is_disabled() && item.children.is_empty() {
                let display = path.strip_prefix(root).unwrap_or(&path);
                paths.push(display.to_string_lossy().replace('\\', "/"));
            }
            collect(&item.children, root, paths);
        }
    }

    let mut paths = Vec::new();
    collect(items, root, &mut paths);
    paths
}

pub fn file_nodes(root: &Path, expanded: &std::collections::BTreeSet<String>) -> Node {
    let mut remaining = MAX_TREE_ENTRIES;
    build_tree_node(root, true, expanded, &mut remaining)
}

fn build_tree_node(
    path: &Path,
    root: bool,
    expanded: &std::collections::BTreeSet<String>,
    remaining: &mut usize,
) -> Node {
    let directory = path.is_dir();
    let mut node = Node {
        path: path_id(path),
        directory,
        children: Vec::new(),
        loaded: root || expanded.contains(&path_id(path)),
    };
    if !directory || !node.loaded || *remaining == 0 {
        return node;
    }
    let mut entries = match fs::read_dir(path) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let kind = entry.file_type().ok()?;
                let name = entry.file_name().to_string_lossy().into_owned();
                (!kind.is_symlink()
                    && !(kind.is_dir() && IGNORED_DIRECTORIES.contains(&name.as_str())))
                .then(|| (entry.path(), kind.is_dir(), name))
            })
            .collect::<Vec<_>>(),
        Err(_) => return node,
    };
    entries.sort_by_key(|(_, directory, name)| (!*directory, name.to_lowercase()));
    for (path, _, _) in entries {
        if *remaining == 0 {
            break;
        }
        *remaining -= 1;
        node.children
            .push(build_tree_node(&path, false, expanded, remaining));
    }
    node
}

pub fn path_id(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub fn language_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "rust",
        "toml" => "toml",
        "md" | "mdx" => "markdown",
        "sh" | "bash" | "zsh" => "bash",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "css" => "css",
        "go" => "go",
        "html" | "htm" => "html",
        "pdf" => "pdf",
        "java" => "java",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" => "typescript",
        "tsx" => "tsx",
        "py" | "pyi" => "python",
        "yaml" | "yml" => "yaml",
        "zig" => "zig",
        "json" | "jsonc" => "json",
        _ => "plaintext",
    }
}

pub fn read_file(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path).with_context(|| {
        crate::t!("file.metadata_failed", path = path.display().to_string()).to_string()
    })?;
    if !metadata.is_file() {
        bail!("{}", crate::t!("file.not_regular_file"));
    }
    if metadata.len() > MAX_FILE_BYTES {
        bail!("{}", crate::t!("file.too_large_open"));
    }
    fs::read_to_string(path).with_context(|| {
        crate::t!("file.not_utf8_path", path = path.display().to_string()).to_string()
    })
}

pub fn write_file(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).with_context(|| {
        crate::t!("file.save_failed", path = path.display().to_string()).to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_editor_languages() {
        assert_eq!(language_for_path(Path::new("src/main.rs")), "rust");
        assert_eq!(language_for_path(Path::new("Cargo.toml")), "toml");
        assert_eq!(language_for_path(Path::new("README.md")), "markdown");
        assert_eq!(language_for_path(Path::new("manual.PDF")), "pdf");
        assert_eq!(language_for_path(Path::new("script.py")), "python");
        assert_eq!(language_for_path(Path::new("config.yaml")), "yaml");
        assert_eq!(language_for_path(Path::new("notes.txt")), "plaintext");
    }

    #[test]
    fn workspace_tree_starts_with_root_children() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        fs::write(root.join("src/nested/deep.rs"), "").unwrap();
        let items = file_tree(root);
        assert!(items.iter().any(|item| item.label == "src"));
        let paths = file_paths(&items, root);
        assert!(paths.iter().any(|path| path == "Cargo.toml"));
        assert!(!paths.iter().any(|path| path == "src/lib.rs"));
        let expanded = [path_id(&root.join("src"))].into_iter().collect();
        let paths = file_paths(
            &file_nodes(root, &expanded).item(true, false).children,
            root,
        );
        assert!(paths.iter().any(|path| path == "src/lib.rs"));
        assert!(!paths.iter().any(|path| path == "src/nested/deep.rs"));
    }

    #[test]
    fn refuses_large_files() {
        let path =
            std::env::temp_dir().join(format!("tshell-large-editor-test-{}", std::process::id()));
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        let error = read_file(&path).unwrap_err().to_string();
        let _ = fs::remove_file(path);
        assert!(error.contains("20 MiB"));
    }

    #[test]
    fn path_ids_round_trip_on_windows() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert_eq!(std::path::PathBuf::from(path_id(&path)), path);
    }
}
