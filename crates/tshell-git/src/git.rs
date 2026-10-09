//! Git output is shared by local and SSH workspaces. Paths stay NUL-delimited.
use tshell_host::SshConfig;
const MAX_FILE_BYTES: u64 = tshell_host::files::MAX_FILE_BYTES;
use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub added: usize,
    pub removed: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitChange {
    pub status: String,
    pub path: PathBuf,
    pub original: Option<PathBuf>,
    pub root: PathBuf,
    pub stats: Option<Stats>,
}

impl GitChange {
    pub fn label(&self) -> &'static str {
        if self.status.contains('U') || matches!(self.status.as_str(), "AA" | "DD") {
            "U"
        } else if self.status.contains('D') {
            "D"
        } else if self.status.contains('R') {
            "R"
        } else if self.status.contains('C') {
            "C"
        } else if self.status.contains('A') || self.status == "??" {
            "A"
        } else {
            "M"
        }
    }
}

fn run(root: &Path, host: Option<&SshConfig>, args: &[&str]) -> Result<Vec<u8>> {
    let mut request = tshell_host::CommandRequest::new(
        "git",
        ["--literal-pathspecs", "-C"]
            .into_iter()
            .chain(std::iter::once(
                root.to_str().context(crate::t!("git.run_failed"))?,
            ))
            .chain(args.iter().copied()),
    );
    request.output_limit = MAX_FILE_BYTES as usize;
    request
        .environment
        .insert("GIT_OPTIONAL_LOCKS".into(), "0".into());
    if args.contains(&"--no-index") {
        request.allowed_status = vec![0, 1];
    }
    tshell_host::Host::from_ssh(host).run(request)
}

fn base(root: &Path, host: Option<&SshConfig>) -> Result<String> {
    let bytes = run(root, host, &["rev-parse", "--verify", "HEAD"])
        .or_else(|_| run(root, host, &["hash-object", "-t", "tree", "--stdin"]))?;
    Ok(String::from_utf8(bytes)?.trim().to_owned())
}

pub fn changes(root: &Path, host: Option<&SshConfig>) -> Result<Vec<GitChange>> {
    let directory = run(root, host, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(String::from_utf8(directory)?.trim_end_matches(['\r', '\n']));
    let status = run(
        &root,
        host,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut changes = parse_git_status(&status);
    for change in &mut changes {
        change.root = root.clone();
        change.path = change_path(&root, &change.path, host.is_some());
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

// SFTP consumes the path verbatim, so remote paths must not use Windows joins.
fn change_path(root: &Path, relative: &Path, remote: bool) -> PathBuf {
    tshell_host::HostPath::for_host(root, remote)
        .join(&relative.to_string_lossy())
        .as_path_buf()
}

/// Only counts are returned here; patch text is reserved for `patch` on click.
pub fn tracked_counts(changes: &mut [GitChange], host: Option<&SshConfig>) -> Result<()> {
    let Some(first) = changes.first() else {
        return Ok(());
    };
    let root = first.root.clone();
    let revision = base(&root, host)?;
    let stats = parse_stats(&run(
        &root,
        host,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--numstat",
            "-z",
            &revision,
            "--",
        ],
    )?);
    for change in changes.iter_mut().filter(|change| change.status != "??") {
        let relative = relative_path(&change.path, &root, host.is_some());
        change.stats = stats
            .get(Path::new(&relative))
            .copied()
            .unwrap_or(Some(Stats {
                added: 0,
                removed: 0,
            }));
    }
    Ok(())
}

/// One SSH round trip for a batch, instead of one round trip per file.
pub fn untracked_counts(changes: &mut [GitChange], host: Option<&SshConfig>) -> Result<()> {
    if let Some(host) = host {
        let commands = changes
            .iter()
            .map(|change| {
                let path = change.path.to_string_lossy();
                let mut request = tshell_host::CommandRequest::new(
                    "git",
                    [
                        "--literal-pathspecs",
                        "-C",
                        &change.root.to_string_lossy(),
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--numstat",
                        "-z",
                        "--no-index",
                        "--",
                        "/dev/null",
                        &path,
                    ],
                );
                request
                    .environment
                    .insert("GIT_OPTIONAL_LOCKS".into(), "0".into());
                request
                    .remote_command()
                    .map(|command| format!("({command}; status=$?; test $status -le 1)"))
            })
            .collect::<Result<Vec<_>>>()?
            .join(" && ");
        if commands.is_empty() {
            return Ok(());
        }
        let stats = parse_stats(&tshell_host::ssh::output(
            host,
            commands,
            MAX_FILE_BYTES as usize,
        )?);
        for change in changes {
            change.stats = stats.get(&change.path).copied().unwrap_or(Some(Stats {
                added: 0,
                removed: 0,
            }));
        }
    } else {
        for change in changes {
            change.stats = parse_stats(&file_diff(change, None, "--numstat")?)
                .into_values()
                .next()
                .unwrap_or(Some(Stats {
                    added: 0,
                    removed: 0,
                }));
        }
    }
    Ok(())
}

pub(crate) fn relative_path(path: &Path, root: &Path, remote: bool) -> String {
    tshell_host::HostPath::for_host(path, remote)
        .relative_to(&tshell_host::HostPath::for_host(root, remote))
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn file_diff(change: &GitChange, host: Option<&SshConfig>, format: &str) -> Result<Vec<u8>> {
    let path = relative_path(&change.path, &change.root, host.is_some());
    let common = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "-z",
        format,
    ];
    let mut args = common.to_vec();
    if format == "--patch" {
        args.push("--unified=2147483647");
    }
    let revision;
    if change.status == "??" {
        args.extend([
            "--no-index",
            "--",
            if host.is_some() || !cfg!(windows) {
                "/dev/null"
            } else {
                "NUL"
            },
            &path,
        ]);
    } else {
        revision = base(&change.root, host)?;
        args.extend([&revision, "--", &path]);
    }
    let original = change.original.as_ref().map(|path| {
        if host.is_some() {
            path.to_string_lossy().into_owned()
        } else {
            path.to_string_lossy().replace('\\', "/")
        }
    });
    if let Some(original) = &original {
        args.push(original);
    }
    run(&change.root, host, &args)
}

pub fn patch(change: &GitChange, host: Option<&SshConfig>) -> Result<Vec<DiffLine>> {
    Ok(parse_patch(&String::from_utf8_lossy(&file_diff(
        change, host, "--patch",
    )?)))
}

fn parse_stats(bytes: &[u8]) -> BTreeMap<PathBuf, Option<Stats>> {
    let mut fields = bytes.split(|b| *b == 0);
    let mut result = BTreeMap::new();
    while let Some(field) = fields.next() {
        let mut parts = field.splitn(3, |b| *b == b'\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            fields.next();
            fields.next().unwrap_or_default()
        } else {
            path
        };
        let count = |s| {
            std::str::from_utf8(s)
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
        };
        let stats = count(added)
            .zip(count(removed))
            .map(|(added, removed)| Stats { added, removed });
        result.insert(
            PathBuf::from(String::from_utf8_lossy(path).into_owned()),
            stats,
        );
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    Header,
}
#[derive(Clone)]
pub struct DiffLine {
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub kind: LineKind,
    pub text: String,
}

pub fn parse_patch(patch: &str) -> Vec<DiffLine> {
    let (mut old, mut new, mut in_hunk) = (0, 0, false);
    patch
        .lines()
        .map(|line| {
            if line.starts_with("diff --git ") {
                in_hunk = false;
            }
            if line.starts_with("@@ ") {
                let mut fields = line.split_whitespace().skip(1);
                let number = |s: &str| {
                    s.get(1..)
                        .and_then(|s| s.split(',').next())
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0)
                };
                old = fields.next().map(number).unwrap_or(0);
                new = fields.next().map(number).unwrap_or(0);
                in_hunk = true;
            } else if in_hunk {
                let (kind, left, right) = match line.as_bytes().first() {
                    Some(b'+') => {
                        new += 1;
                        (LineKind::Added, None, Some(new - 1))
                    }
                    Some(b'-') => {
                        old += 1;
                        (LineKind::Removed, Some(old - 1), None)
                    }
                    Some(b' ') => {
                        old += 1;
                        new += 1;
                        (LineKind::Context, Some(old - 1), Some(new - 1))
                    }
                    _ => (LineKind::Header, None, None),
                };
                return DiffLine {
                    old: left,
                    new: right,
                    kind,
                    text: line.to_owned(),
                };
            }
            DiffLine {
                old: None,
                new: None,
                kind: LineKind::Header,
                text: line.to_owned(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn git_porcelain_status_handles_spaces_and_rename_source() {
        let changes = parse_git_status(
            b" M src/workspace.rs\0?? new file.txt\0R  new name.rs\0old name.rs\0",
        );
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].status, " M");
        assert_eq!(changes[1].path, PathBuf::from("new file.txt"));
        assert_eq!(changes[2].status, "R ");
        assert_eq!(changes[2].path, PathBuf::from("new name.rs"));
    }
    use super::*;
    fn with_counts(root: &Path, host: Option<&SshConfig>) -> Result<Vec<GitChange>> {
        let mut changes = changes(root, host)?;
        assert!(changes.iter().all(|change| change.stats.is_none()));
        tracked_counts(&mut changes, host)?;
        for change in changes.iter_mut().filter(|change| change.status == "??") {
            untracked_counts(std::slice::from_mut(change), host)?;
        }
        Ok(changes)
    }
    #[test]
    fn repository_diff_covers_unborn_staged_worktree_and_deleted_files() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "tshell-git-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir(&root)?;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        run(&root, None, &["init"])?;
        std::fs::write(root.join("new ' file.txt"), "one\ntwo\n")?;
        std::fs::write(root.join("empty"), "")?;
        std::fs::write(root.join("binary"), [0, 1, 2])?;
        let original = (1..=40)
            .map(|number| format!("line {number}\n"))
            .collect::<String>();
        std::fs::write(root.join("context.txt"), &original)?;
        let unborn = with_counts(&root, None)?;
        let new_file = unborn
            .iter()
            .find(|c| c.path.ends_with("new ' file.txt"))
            .unwrap();
        assert_eq!(
            new_file.stats,
            Some(Stats {
                added: 2,
                removed: 0
            })
        );
        assert_eq!(
            patch(new_file, None)?
                .iter()
                .filter(|line| line.kind == LineKind::Added)
                .count(),
            2
        );
        assert_eq!(
            unborn
                .iter()
                .find(|c| c.path.ends_with("empty"))
                .unwrap()
                .stats,
            Some(Stats {
                added: 0,
                removed: 0
            })
        );
        assert_eq!(
            unborn
                .iter()
                .find(|c| c.path.ends_with("binary"))
                .unwrap()
                .stats,
            None
        );
        run(&root, None, &["add", "."])?;
        assert_eq!(
            with_counts(&root, None)?
                .iter()
                .find(|c| c.path.ends_with("new ' file.txt"))
                .unwrap()
                .stats,
            new_file.stats
        );
        run(
            &root,
            None,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "baseline",
            ],
        )?;
        std::fs::write(root.join("new ' file.txt"), "one\nstaged\n")?;
        run(&root, None, &["add", "."])?;
        std::fs::write(root.join("new ' file.txt"), "one\nworking\nextra\n")?;
        let updated = original.replace("line 20\n", "changed 20\n");
        std::fs::write(root.join("context.txt"), &updated)?;
        std::fs::remove_file(root.join("binary"))?;
        run(&root, None, &["mv", "empty", "renamed"])?;
        let changed = with_counts(&root, None)?;
        let context = changed
            .iter()
            .find(|change| change.path.ends_with("context.txt"))
            .unwrap();
        let full = patch(context, None)?;
        let before = full
            .iter()
            .filter(|line| line.old.is_some())
            .map(|line| format!("{}\n", &line.text[1..]))
            .collect::<String>();
        let after = full
            .iter()
            .filter(|line| line.new.is_some())
            .map(|line| format!("{}\n", &line.text[1..]))
            .collect::<String>();
        assert_eq!(before, original);
        assert_eq!(after, updated);
        let modified = changed
            .iter()
            .find(|c| c.path.ends_with("new ' file.txt"))
            .unwrap();
        assert_eq!(modified.status, "MM");
        assert_eq!(
            modified.stats,
            Some(Stats {
                added: 2,
                removed: 1
            })
        );
        let lines = patch(modified, None)?;
        assert!(lines.iter().any(|line| line.text == "+working"));
        assert!(!lines.iter().any(|line| line.text == "+staged"));
        let renamed = changed.iter().find(|c| c.label() == "R").unwrap();
        assert!(
            patch(renamed, None)?
                .iter()
                .any(|line| line.text.starts_with("rename from"))
        );
        assert_eq!(
            changed.iter().find(|c| c.label() == "D").unwrap().stats,
            None
        );
        Ok(())
    }
    #[test]
    fn remote_paths_keep_posix_separators_for_sftp() {
        let path = change_path(
            Path::new("/srv/my project"),
            Path::new("src/new file.rs"),
            true,
        );
        assert_eq!(path.to_string_lossy(), "/srv/my project/src/new file.rs");
    }

    #[test]
    fn stats_preserve_renames_tabs_and_binary() {
        let stats = parse_stats(b"3\t2\t\0old name\0new\tname\0-\t-\timage.png\0");
        assert_eq!(
            stats[Path::new("new\tname")],
            Some(Stats {
                added: 3,
                removed: 2
            })
        );
        assert_eq!(stats[Path::new("image.png")], None);
    }
    #[test]
    fn hunks_track_both_line_numbers_and_header_like_content() {
        let rows = parse_patch(
            "--- a/file\n+++ b/file\n@@ -3,2 +7,2 @@\n---old\n+++new\n same\n\\ No newline at end of file\n@@ -20,0 +25 @@\n+tail\n",
        );
        assert_eq!(
            (rows[3].old, rows[3].new, rows[3].kind),
            (Some(3), None, LineKind::Removed)
        );
        assert_eq!((rows[4].old, rows[4].new), (None, Some(7)));
        assert_eq!((rows[5].old, rows[5].new), (Some(4), Some(8)));
        assert_eq!(rows[8].new, Some(25));
    }
}

pub fn parse_git_status(bytes: &[u8]) -> Vec<GitChange> {
    let fields = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let field = fields[index];
        if field.is_empty() {
            break;
        }
        if field.len() < 4 {
            index += 1;
            continue;
        }
        let status = String::from_utf8_lossy(&field[..2]).into_owned();
        let path = PathBuf::from(String::from_utf8_lossy(&field[3..]).into_owned());
        let original = if matches!(field[0], b'R' | b'C') || matches!(field[1], b'R' | b'C') {
            fields
                .get(index + 1)
                .map(|value| PathBuf::from(String::from_utf8_lossy(value).into_owned()))
        } else {
            None
        };
        changes.push(GitChange {
            status,
            path,
            original,
            root: PathBuf::new(),
            stats: None,
        });
        if matches!(field[0], b'R' | b'C') || matches!(field[1], b'R' | b'C') {
            index += 1;
        }
        index += 1;
    }
    changes
}
