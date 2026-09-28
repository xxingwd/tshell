//! Git output is shared by local and SSH workspaces. Paths stay NUL-delimited.
use super::{HostConfig, workbench};
use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stats {
    pub added: usize,
    pub removed: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GitChange {
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

fn remote_command(root: &Path, args: &[&str]) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let words = args
        .iter()
        .map(|arg| quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let command = format!(
        "GIT_OPTIONAL_LOCKS=0 git --literal-pathspecs -C {} {words} </dev/null",
        quote(&root.to_string_lossy())
    );
    if args.contains(&"--no-index") {
        format!("( {command}; code=$?; [ \"$code\" -le 1 ] )")
    } else {
        command
    }
}

fn run(root: &Path, host: Option<&HostConfig>, args: &[&str]) -> Result<Vec<u8>> {
    if let Some(host) = host {
        return crate::ssh_pool::output(
            host,
            remote_command(root, args),
            workbench::MAX_FILE_BYTES as usize,
        );
    }
    let output = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context(crate::t!("git.run_failed"))?;
    anyhow::ensure!(
        output.status.success()
            || (args.contains(&"--no-index") && output.status.code() == Some(1)),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    anyhow::ensure!(
        output.stdout.len() <= workbench::MAX_FILE_BYTES as usize,
        "{}",
        crate::t!("git.output_too_large")
    );
    Ok(output.stdout)
}

fn base(root: &Path, host: Option<&HostConfig>) -> Result<String> {
    let bytes = run(root, host, &["rev-parse", "--verify", "HEAD"])
        .or_else(|_| run(root, host, &["hash-object", "-t", "tree", "--stdin"]))?;
    Ok(String::from_utf8(bytes)?.trim().to_owned())
}

pub(super) fn changes(root: &Path, host: Option<&HostConfig>) -> Result<Vec<GitChange>> {
    let directory = run(root, host, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(String::from_utf8(directory)?.trim_end_matches(['\r', '\n']));
    let status = run(
        &root,
        host,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut changes = workbench::parse_git_status(&status);
    for change in &mut changes {
        change.root = root.clone();
        change.path = change_path(&root, &change.path, host.is_some());
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

// SFTP consumes the path verbatim, so remote paths must not use Windows joins.
fn change_path(root: &Path, relative: &Path, remote: bool) -> PathBuf {
    if remote {
        PathBuf::from(format!(
            "{}/{}",
            root.to_string_lossy().trim_end_matches('/'),
            relative.to_string_lossy()
        ))
    } else {
        root.join(relative)
    }
}

/// Only counts are returned here; patch text is reserved for `patch` on click.
pub(super) fn tracked_counts(changes: &mut [GitChange], host: Option<&HostConfig>) -> Result<()> {
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
        let relative = change.path.strip_prefix(&root).unwrap_or(&change.path);
        change.stats = stats.get(relative).copied().unwrap_or(Some(Stats {
            added: 0,
            removed: 0,
        }));
    }
    Ok(())
}

/// One SSH round trip for a batch, instead of one round trip per file.
pub(super) fn untracked_counts(changes: &mut [GitChange], host: Option<&HostConfig>) -> Result<()> {
    if let Some(host) = host {
        let commands = changes
            .iter()
            .map(|change| {
                let path = change.path.to_string_lossy().replace('\\', "/");
                remote_command(
                    &change.root,
                    &[
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
                )
            })
            .collect::<Vec<_>>()
            .join(" && ");
        if commands.is_empty() {
            return Ok(());
        }
        let stats = parse_stats(&crate::ssh_pool::output(
            host,
            commands,
            workbench::MAX_FILE_BYTES as usize,
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

fn file_diff(change: &GitChange, host: Option<&HostConfig>, format: &str) -> Result<Vec<u8>> {
    let path = change
        .path
        .strip_prefix(&change.root)
        .unwrap_or(&change.path)
        .to_string_lossy()
        .replace('\\', "/");
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
    let original = change
        .original
        .as_ref()
        .map(|path| path.to_string_lossy().replace('\\', "/"));
    if let Some(original) = &original {
        args.push(original);
    }
    run(&change.root, host, &args)
}

pub(super) fn patch(change: &GitChange, host: Option<&HostConfig>) -> Result<Vec<DiffLine>> {
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
pub(super) enum LineKind {
    Context,
    Added,
    Removed,
    Header,
}
#[derive(Clone)]
pub(super) struct DiffLine {
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub kind: LineKind,
    pub text: String,
}

pub(super) fn parse_patch(patch: &str) -> Vec<DiffLine> {
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
    use super::*;
    fn with_counts(root: &Path, host: Option<&HostConfig>) -> Result<Vec<GitChange>> {
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
