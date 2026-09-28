use super::*;

#[derive(Clone)]
enum SessionRef {
    Live(String),
    Saved(crate::backend::SessionProfile),
}

#[derive(Clone)]
pub(super) struct SessionTarget {
    host: usize,
    session: SessionRef,
    name: String,
    path: String,
}

impl AppView {
    pub(super) fn launcher_sessions(&self) -> Vec<SessionTarget> {
        self.hosts
            .iter()
            .enumerate()
            .flat_map(|(host, entry)| {
                if entry.backend.is_some() || !entry.snapshot.sessions.is_empty() {
                    entry
                        .snapshot
                        .sessions
                        .iter()
                        .map(|session| SessionTarget {
                            host,
                            session: SessionRef::Live(session.id.clone()),
                            name: session.name.clone(),
                            path: session.cwd.clone(),
                        })
                        .collect::<Vec<_>>()
                } else {
                    session_storage_key(entry.config.as_ref())
                        .and_then(|key| self.session_profiles.get(&key))
                        .into_iter()
                        .flatten()
                        .map(|profile| SessionTarget {
                            host,
                            session: SessionRef::Saved(profile.clone()),
                            name: profile.name.clone(),
                            path: profile.path.clone(),
                        })
                        .collect()
                }
            })
            .collect()
    }

    pub(super) fn launcher_session_item(&self, target: &SessionTarget) -> CommandItem {
        let host = &self.hosts[target.host];
        CommandItem::new()
            .label(format!("{} / {}", host.name, target.name))
            .icon(IconName::Terminal)
            .keywords([
                target.name.clone(),
                target.path.clone(),
                host.name.clone(),
                host.config
                    .as_ref()
                    .map(|config| config.destination.clone())
                    .unwrap_or_else(|| crate::t!("ws.local_keywords").to_string()),
            ])
    }

    pub(super) fn launch_session(
        &mut self,
        target: SessionTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if target.host >= self.hosts.len() {
            return;
        }
        self.close_modal(window, cx);
        self.switch_host(target.host, cx);
        let host = &self.hosts[target.host];
        let id = match target.session {
            SessionRef::Live(id) => Some(id),
            SessionRef::Saved(profile) => host
                .snapshot
                .sessions
                .iter()
                .find(|session| session.name == profile.name && session.cwd == profile.path)
                .map(|session| session.id.clone()),
        };
        if let Some(id) = id {
            if host.snapshot.connected() {
                if host
                    .snapshot
                    .sessions
                    .iter()
                    .any(|session| session.id == id)
                {
                    self.act(Action::SelectSession(id), cx);
                } else {
                    self.message = Some(crate::t!("launcher.closed").to_string());
                }
            } else {
                self.hosts[target.host]
                    .pending
                    .push(Action::SelectSession(id));
            }
        }
        self.show_terminal(cx);
    }
}
