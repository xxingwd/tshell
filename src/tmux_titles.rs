//! Title metadata from tmux's ordered raw %output stream. This uses the same
//! ANSI processor as the terminal, including fragmented OSC and title stacks.
use crate::backend::Snapshot;
use alacritty_terminal::vte::ansi::{Handler, Processor};
use std::collections::BTreeMap;

#[derive(Default)]
struct Title {
    value: Option<String>,
    stack: Vec<Option<String>>,
    version: u64,
}
impl Handler for Title {
    fn set_title(&mut self, value: Option<String>) {
        if self.value != value {
            self.value = value;
            self.version += 1;
        }
    }
    fn push_title(&mut self) {
        if self.stack.len() == 4096 {
            self.stack.remove(0);
        }
        self.stack.push(self.value.clone());
    }
    fn pop_title(&mut self) {
        if let Some(value) = self.stack.pop() {
            self.set_title(value);
        }
    }
    fn reset_state(&mut self) {
        self.stack.clear();
        self.set_title(None);
    }
}
#[derive(Default)]
struct PaneTitle {
    parser: Processor,
    notifications: crate::terminal_notifications::Notifications,
    clipboard: crate::terminal_clipboard::Clipboard,
    title: Title,
    queried_version: u64,
    notice_count: u64,
    path: Option<String>,
    path_version: u64,
    queried_path_version: u64,
}
#[derive(Default)]
pub(crate) struct LiveTitles(BTreeMap<String, PaneTitle>);
impl LiveTitles {
    pub fn advance(&mut self, id: &str, bytes: &[u8], snapshot: &mut Snapshot) {
        let pane = self.0.entry(id.to_owned()).or_default();
        pane.clipboard.advance(bytes);
        let notices = pane.notifications.advance(bytes);
        pane.notice_count += notices;
        let before = pane.title.version;
        pane.parser.advance(&mut pane.title, bytes);
        if before == pane.title.version && notices == 0 {
            return;
        }
        let fallback = crate::t!("term.shell");
        let title = pane.title.value.as_deref().unwrap_or(fallback.as_ref());
        let mut changed = false;
        for p in snapshot
            .sessions
            .iter_mut()
            .flat_map(|s| &mut s.windows)
            .flat_map(|w| &mut w.panes)
        {
            if p.id == id {
                p.notice_count = pane.notice_count;
                if before != pane.title.version {
                    p.title = title.into();
                }
                changed = true;
            }
        }
        if changed {
            snapshot.revision += 1;
        }
    }
    /// Apply tmux format notifications without querying the entire layout.
    /// Versions prevent an older in-flight query from overwriting this event.
    pub fn subscription(&mut self, line: &str, snapshot: &mut Snapshot) -> bool {
        let Some((header, value)) = line.split_once(" : ") else {
            return false;
        };
        let mut fields = header.split_whitespace();
        if fields.next() != Some("%subscription-changed") {
            return false;
        }
        let Some(name @ ("tshell-titles" | "tshell-paths")) = fields.next() else {
            return false;
        };
        let Some(id) = fields.nth(3).filter(|id| id.starts_with('%')) else {
            return false;
        };
        let Ok(values) = shell_words::split(value) else {
            return false;
        };
        let value = match values.as_slice() {
            [] => String::new(),
            [value] => value.clone(),
            _ => return false,
        };
        let pane = self.0.entry(id.to_owned()).or_default();
        if name == "tshell-titles" {
            pane.title.set_title(Some(value.clone()));
        } else if pane.path.as_ref() != Some(&value) {
            pane.path = Some(value.clone());
            pane.path_version += 1;
        }
        let mut changed = false;
        for p in snapshot
            .sessions
            .iter_mut()
            .flat_map(|s| &mut s.windows)
            .flat_map(|w| &mut w.panes)
            .filter(|p| p.id == id)
        {
            let field = if name == "tshell-titles" {
                &mut p.title
            } else {
                &mut p.cwd
            };
            if *field != value {
                *field = value.clone();
                changed = true;
            }
        }
        if changed {
            snapshot.revision += 1;
        }
        true
    }

    pub fn begin_query(&mut self) {
        for pane in self.0.values_mut() {
            pane.queried_version = pane.title.version;
            pane.queried_path_version = pane.path_version;
        }
    }
    pub fn reconcile(&mut self, snapshot: &mut Snapshot) {
        for p in snapshot
            .sessions
            .iter_mut()
            .flat_map(|s| &mut s.windows)
            .flat_map(|w| &mut w.panes)
        {
            let pane = self.0.entry(p.id.clone()).or_default();
            p.notice_count = pane.notice_count;
            if pane.path_version != pane.queried_path_version {
                if let Some(path) = &pane.path {
                    p.cwd = path.clone();
                }
            } else {
                pane.path = Some(p.cwd.clone());
            }
            if pane.title.version != pane.queried_version {
                // Output received after the query began is newer than its response.
                p.title = pane
                    .title
                    .value
                    .clone()
                    .unwrap_or_else(|| crate::t!("term.shell").to_string());
            } else {
                // Allow external tmux select-pane -T changes and initial titles.
                pane.title.value = Some(p.title.clone());
            }
        }
    }
    pub fn retain(&mut self, ids: &std::collections::BTreeSet<String>) {
        self.0.retain(|id, _| ids.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{PaneInfo, SessionInfo, WindowInfo};
    fn snapshot() -> Snapshot {
        Snapshot {
            sessions: vec![SessionInfo {
                windows: vec![WindowInfo {
                    panes: vec![PaneInfo {
                        id: "%1".into(),
                        title: "old".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }
    fn title(s: &Snapshot) -> &str {
        &s.sessions[0].windows[0].panes[0].title
    }
    #[test]
    fn streamed_titles_survive_fragmentation_and_stale_queries() {
        let mut live = LiveTitles::default();
        let mut s = snapshot();
        live.reconcile(&mut s);
        live.begin_query();
        live.advance("%1", b"\x1b]2;new", &mut s);
        assert_eq!(title(&s), "old");
        live.advance("%1", b" title\x1b\\", &mut s);
        assert_eq!(title(&s), "new title");
        assert_eq!(s.revision, 1);
        let mut stale = snapshot();
        live.reconcile(&mut stale);
        assert_eq!(title(&stale), "new title");
        live.advance("%1", b"\x1b]0;new title\x07", &mut s);
        assert_eq!(s.revision, 1);
        live.begin_query();
        let mut external = snapshot();
        live.reconcile(&mut external);
        assert_eq!(title(&external), "old");
    }
    #[test]
    fn subscription_updates_one_pane_and_survives_an_older_query() {
        let mut live = LiveTitles::default();
        let mut current = snapshot();
        live.reconcile(&mut current);
        live.begin_query();
        assert!(live.subscription(
            "%subscription-changed tshell-titles $0 @0 0 %1 : 中文\\ title",
            &mut current
        ));
        assert!(live.subscription(
            "%subscription-changed tshell-paths $0 @0 0 %1 : /home/test\\ dir",
            &mut current
        ));
        assert_eq!(title(&current), "中文 title");
        assert_eq!(
            current.sessions[0].windows[0].panes[0].cwd,
            "/home/test dir"
        );
        let revision = current.revision;
        assert!(live.subscription(
            "%subscription-changed tshell-paths $0 @0 0 %1 : /home/test\\ dir",
            &mut current
        ));
        assert_eq!(revision, current.revision);
        let mut stale = snapshot();
        live.reconcile(&mut stale);
        assert_eq!(title(&stale), "中文 title");
        assert_eq!(stale.sessions[0].windows[0].panes[0].cwd, "/home/test dir");
        assert!(!live.subscription("%layout-change @0 data", &mut current));
    }

    #[test]
    fn title_stack_and_background_output_use_latest_value() {
        let mut live = LiveTitles::default();
        let mut s = snapshot();
        live.advance(
            "%1",
            b"\x1b]2;first\x07\x1b[22;2t\x1b]2;second\x07\x1b[23;2t",
            &mut s,
        );
        assert_eq!(title(&s), "first");
        live.begin_query();
        live.advance("%2", b"\x1b]2;background\x07", &mut s);
        s.sessions[0].windows[0].panes[0].id = "%2".into();
        live.reconcile(&mut s);
        assert_eq!(title(&s), "background");
    }
}
