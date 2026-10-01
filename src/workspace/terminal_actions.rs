use super::*;
use crate::terminal::links::{LinkTarget, OpenLink};

struct TerminalLocation {
    host: usize,
    session: String,
    tab: String,
    pane: String,
    directory: String,
}

impl AppView {
    fn terminal_location(&self, identity: u64) -> Option<TerminalLocation> {
        for (index, host) in self.hosts.iter().enumerate() {
            let Some(backend) = &host.backend else {
                continue;
            };
            for session in &host.snapshot.sessions {
                for tab in &session.windows {
                    for pane in &tab.panes {
                        if backend
                            .screen(&pane.id)
                            .is_some_and(|screen| screen.identity == identity)
                        {
                            return Some(TerminalLocation {
                                host: index,
                                session: session.id.clone(),
                                tab: tab.id.clone(),
                                pane: pane.id.clone(),
                                directory: pane.cwd.clone(),
                            });
                        }
                    }
                }
            }
        }
        None
    }

    pub(super) fn subscribe_terminal_links(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let views: Vec<_> = self
            .hosts
            .iter()
            .flat_map(|host| host.views.values())
            .filter(|view| !view.read(cx).link_subscribed)
            .cloned()
            .collect();
        for view in views {
            view.update(cx, |view, _| view.link_subscribed = true);
            cx.subscribe_in(&view, window, |this, _, link: &OpenLink, window, cx| {
                this.open_terminal_link(link.clone(), window, cx)
            })
            .detach();
        }
    }

    pub(super) fn focus_terminal_notice(
        &mut self,
        identity: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.activate_window();
        cx.activate(true);
        self.sync(cx);
        let Some(location) = identity.and_then(|identity| self.terminal_location(identity)) else {
            window.push_notification(Notification::warning(crate::t!("term.notice_closed")), cx);
            return;
        };
        if self.settings || self.command_palette || self.creating_tab {
            self.close_modal(window, cx);
        }
        self.hosts[location.host]
            .collapsed_sessions
            .remove(&location.session);
        self.switch_host(location.host, cx);
        self.select_terminal(location.session, location.tab, Some(location.pane), cx);
        self.sync(cx);
        self.need_focus = true;
        cx.notify();
    }

    pub(super) fn open_terminal_link(
        &mut self,
        link: OpenLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match link.target {
            LinkTarget::Url(url) => cx.open_url(url.as_str()),
            LinkTarget::File {
                path,
                host,
                line,
                column,
            } => {
                self.sync(cx);
                let Some(location) = self.terminal_location(link.identity) else {
                    window.push_notification(
                        Notification::warning(crate::t!("term.notice_closed")),
                        cx,
                    );
                    return;
                };
                let remote = self.hosts[location.host].config.is_some();
                let directory = link
                    .directory
                    .as_deref()
                    .filter(|path| !path.is_empty())
                    .unwrap_or(&location.directory);
                let path = resolve_link_path(&path, host.as_deref(), directory, remote);
                self.switch_host(location.host, cx);
                self.select_terminal(location.session, location.tab, Some(location.pane), cx);
                self.sync(cx);
                let position = line.map(|line| {
                    (
                        (line - 1).min(u32::MAX as usize) as u32,
                        column.unwrap_or(1).saturating_sub(1).min(u32::MAX as usize) as u32,
                    )
                });
                self.open_terminal_path(path, position, window, cx);
            }
        }
    }

    fn open_terminal_path(
        &mut self,
        path: PathBuf,
        position: Option<(u32, u32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_request += 1;
        let request = self.file_request;
        let key = self.active_file_session.clone();
        let remote = self.remote_files();
        self.file_loading = Some(crate::t!("file.reading_file").to_string());
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match remote {
                        Some(remote) => remote_files::inspect_path(&remote, &path),
                        None => std::fs::metadata(&path)
                            .map(|metadata| (path, metadata.is_dir()))
                            .map_err(anyhow::Error::from),
                    }
                })
                .await;
            let _ = view.update_in(cx, |this, window, cx| {
                if this.file_request != request || this.active_file_session != key {
                    return;
                }
                this.file_loading = None;
                match result {
                    Ok((path, false)) => this.open_path_at(path, position, window, cx),
                    Ok((path, true)) => {
                        if let Some(key) = key {
                            this.tool_roots.insert(key, path.clone());
                        }
                        this.cwd = path;
                        this.tool_request += 1;
                        this.show_files(cx);
                    }
                    Err(error) => {
                        window.push_notification(Notification::error(format!("{error:#}")), cx)
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn resolve_link_path(path: &str, host: Option<&str>, directory: &str, remote: bool) -> PathBuf {
    if remote {
        if path.starts_with('/') || path.starts_with("~/") {
            path.into()
        } else {
            format!("{}/{}", directory.trim_end_matches('/'), path).into()
        }
    } else {
        let path =
            if cfg!(windows) && path.as_bytes().get(2) == Some(&b':') && path.starts_with('/') {
                &path[1..]
            } else {
                path
            };
        if let Some(host) =
            host.filter(|host| !host.is_empty() && !host.eq_ignore_ascii_case("localhost"))
        {
            if cfg!(windows) {
                return format!("\\\\{host}{}", path.replace('/', "\\")).into();
            }
        }
        if let Some(relative) = path.strip_prefix("~/") {
            return dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from(directory))
                .join(relative);
        }
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            PathBuf::from(directory).join(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn ssh_paths_follow_emitting_host_and_posix_directory_on_windows() {
        assert_eq!(
            resolve_link_path("./src/a.rs", None, "/srv/app", true).to_string_lossy(),
            "/srv/app/./src/a.rs"
        );
        assert_eq!(
            resolve_link_path("/home/a b.txt", Some("actual-remote-name"), "/srv", true)
                .to_string_lossy(),
            "/home/a b.txt"
        );
        assert_eq!(
            resolve_link_path("~/a.txt", None, "/srv", true).to_string_lossy(),
            "~/a.txt"
        );
        #[cfg(windows)]
        assert_eq!(
            resolve_link_path("/C:/work/a.rs", None, "C:/other", false),
            PathBuf::from("C:/work/a.rs")
        );
    }
}
