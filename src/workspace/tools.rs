//! Per-tab, in-memory file tools. tmux remains the source of pane directories.
use super::*;

impl AppView {
    pub(super) fn show_files(&mut self, cx: &mut Context<Self>) {
        self.open_tools(WorkspaceMode::Files, cx);
    }
    pub(super) fn show_git(&mut self, cx: &mut Context<Self>) {
        self.open_tools(WorkspaceMode::Git, cx);
    }
    pub(super) fn show_terminal(&mut self, cx: &mut Context<Self>) {
        self.workspace_mode = WorkspaceMode::Terminal;
        self.need_focus = true;
        cx.notify();
    }
    pub(super) fn open_tools(&mut self, mode: WorkspaceMode, cx: &mut Context<Self>) {
        let Some(key) = self.active_file_session.clone() else {
            return;
        };
        self.workspace_mode = mode;
        self.need_focus = true;
        if self.tool_roots.contains_key(&key) {
            self.refresh_tools(cx);
            return;
        }
        let host = &self.hosts[self.active];
        let Some(tab) = host.snapshot.window() else {
            return;
        };
        let pane = tab.active_pane.clone();
        let path = tab
            .panes
            .iter()
            .find(|p| p.id == pane)
            .map(|p| p.cwd.clone())
            .filter(|p| !p.is_empty());
        let local = host
            .backend
            .as_ref()
            .and_then(|b| b.screen(&pane))
            .and_then(|s| s.current_directory());
        let fallback = host
            .snapshot
            .session()
            .map(|s| s.cwd.clone())
            .filter(|p| !p.is_empty());
        let remote = self.remote_files();
        self.tool_request += 1;
        let request = self.tool_request;
        self.tree_request += 1;
        self.file_tree
            .update(cx, |tree, cx| tree.set_items(Vec::new(), cx));
        self.git_request += 1;
        self.git_changes.clear();
        self.git_diff = None;
        self.git_diff_request += 1;
        self.file_loading = Some(crate::t!("tools.reading_terminal_directory").to_string());
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(host) = remote {
                        if host.tmux {
                            let command = format!(
                                "{} display-message -p -t '{}' '#{{pane_current_path}}'",
                                host.tmux()?,
                                pane.replace('\'', "'\\''")
                            );
                            let bytes = crate::ssh_pool::output(&host, command, 65536)?;
                            let path = String::from_utf8(bytes)?
                                .trim_end_matches(['\r', '\n'])
                                .to_owned();
                            anyhow::ensure!(
                                !path.is_empty(),
                                "{}",
                                crate::t!("tools.tmux_no_pane_directory")
                            );
                            Ok(PathBuf::from(path))
                        } else {
                            let path = local.or(path).or(fallback).unwrap_or_else(|| "~".into());
                            remote_files::directory(&host, &path).map(PathBuf::from)
                        }
                    } else {
                        Ok(local
                            .or(path)
                            .or(fallback)
                            .map(PathBuf::from)
                            .or_else(dirs::home_dir)
                            .unwrap_or_default())
                    }
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                if this.tool_request != request || this.active_file_session.as_ref() != Some(&key) {
                    return;
                }
                match result {
                    Ok(root) => {
                        this.cwd = root.clone();
                        this.tool_roots.insert(key, root);
                        this.file_loading = None;
                        this.refresh_tools(cx);
                    }
                    Err(error) => {
                        this.file_loading = None;
                        this.message = Some(
                            crate::t!("tools.read_directory_failed", error = format!("{error:#}"))
                                .to_string(),
                        );
                        this.git_error = Some(format!("{error:#}"));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn refresh_tools(&mut self, cx: &mut Context<Self>) {
        match self.workspace_mode {
            WorkspaceMode::Files => self.load_files(cx),
            WorkspaceMode::Git => self.refresh_git(cx),
            WorkspaceMode::Terminal => {}
        }
        cx.notify();
    }
    pub(super) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        if self.active_file_session.is_none() || self.hosts[self.active].backend.is_none() {
            return;
        }
        self.git_request += 1;
        let request = self.git_request;
        let root = self.cwd.clone();
        let host = self.hosts[self.active].config.clone();
        self.git_error = Some(crate::t!("git.loading").to_string());
        self.git_diff = None;
        self.git_diff_request += 1;
        cx.spawn(async move |view, cx| {
            let source = host.clone();
            let result = cx
                .background_executor()
                .spawn(async move { git::changes(&root, source.as_ref()) })
                .await;
            let Some(changes) = view
                .update(cx, |this, cx| {
                    if this.git_request != request {
                        return None;
                    }
                    match result {
                        Ok(changes) => {
                            this.git_changes = changes.clone();
                            this.git_error = None;
                            cx.notify();
                            Some(changes)
                        }
                        Err(error) => {
                            this.git_changes.clear();
                            this.git_error = Some(format!("{error:#}"));
                            cx.notify();
                            None
                        }
                    }
                })
                .ok()
                .flatten()
            else {
                return;
            };
            let untracked = changes
                .iter()
                .enumerate()
                .filter(|(_, change)| change.status == "??")
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            let source = host.clone();
            let (changes, result) = cx
                .background_executor()
                .spawn(async move {
                    let mut changes = changes;
                    let result = git::tracked_counts(&mut changes, source.as_ref());
                    (changes, result)
                })
                .await;
            if !view
                .update(cx, |this, cx| {
                    if this.git_request != request {
                        return false;
                    }
                    for (current, updated) in this.git_changes.iter_mut().zip(&changes) {
                        current.stats = updated.stats;
                    }
                    if let Err(error) = result {
                        this.git_error = Some(
                            crate::t!("tools.line_count_failed", error = format!("{error:#}"))
                                .to_string(),
                        );
                    }
                    cx.notify();
                    true
                })
                .unwrap_or(false)
            {
                return;
            }
            for indices in untracked.chunks(32) {
                let mut batch = indices
                    .iter()
                    .map(|index| changes[*index].clone())
                    .collect::<Vec<_>>();
                let source = host.clone();
                let (batch, result) = cx
                    .background_executor()
                    .spawn(async move {
                        let result = git::untracked_counts(&mut batch, source.as_ref());
                        (batch, result)
                    })
                    .await;
                if !view
                    .update(cx, |this, cx| {
                        if this.git_request != request {
                            return false;
                        }
                        for (index, change) in indices.iter().zip(batch) {
                            this.git_changes[*index].stats = change.stats;
                        }
                        if let Err(error) = result {
                            this.git_error = Some(
                                crate::t!(
                                    "tools.added_line_count_failed",
                                    error = format!("{error:#}")
                                )
                                .to_string(),
                            );
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false)
                {
                    return;
                }
            }
        })
        .detach();
        cx.notify();
    }
}
