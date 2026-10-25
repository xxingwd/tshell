use super::*;
use crate::backend::SessionInfo;

#[cfg(debug_assertions)]
mod ui_check;
#[cfg(debug_assertions)]
pub(super) use ui_check::check;

fn ordered_indices(keys: &[String], order: &[String]) -> Vec<usize> {
    let mut indices: Vec<_> = (0..keys.len()).collect();
    indices.sort_by_key(|&index| {
        order
            .iter()
            .position(|key| key == &keys[index])
            .unwrap_or(usize::MAX)
    });
    indices
}

pub(super) fn apply_session_order(sessions: &mut [SessionInfo], order: &[String]) {
    sessions.sort_by_cached_key(|session| {
        order
            .iter()
            .position(|candidate| candidate == &session.order_id)
            .unwrap_or(usize::MAX)
    });
}

pub(super) fn prune_session_order(order: &mut Vec<String>, snapshot: &Snapshot) -> bool {
    // A disconnected or still-connecting host has no authoritative session list.
    if !snapshot.connected() {
        return false;
    }
    let previous_len = order.len();
    order.retain(|key| {
        snapshot
            .sessions
            .iter()
            .any(|session| &session.order_id == key)
    });
    order.len() != previous_len
}

fn move_item<T>(items: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from == to || from >= items.len() || to >= items.len() {
        return false;
    }
    let item = items.remove(from);
    items.insert(to, item);
    true
}

impl AppView {
    pub(super) fn prune_host_orders(&mut self) -> bool {
        let host_ids: BTreeSet<_> = self
            .hosts
            .iter()
            .map(|host| host.identity().to_string())
            .collect();
        let previous_len = self.host_order.len() + self.session_order.len();
        self.host_order.retain(|id| host_ids.contains(id));
        self.session_order.retain(|id, _| host_ids.contains(id));
        self.host_order.len() + self.session_order.len() != previous_len
    }
    pub(super) fn ordered_host_indices(&self) -> Vec<usize> {
        let keys: Vec<_> = self
            .hosts
            .iter()
            .map(|host| host.identity().to_string())
            .collect();
        ordered_indices(&keys, &self.host_order)
    }

    pub(super) fn move_host(&mut self, index: usize, delta: isize, cx: &mut Context<Self>) {
        let mut indices = self.ordered_host_indices();
        let Some(from) = indices.iter().position(|&candidate| candidate == index) else {
            return;
        };
        let Some(to) = from.checked_add_signed(delta) else {
            return;
        };
        if move_item(&mut indices, from, to) {
            self.host_order = indices
                .iter()
                .map(|&index| self.hosts[index].identity().to_string())
                .collect();
            self.save(cx);
            cx.notify();
        }
    }

    pub(super) fn move_session(
        &mut self,
        host_id: &tshell_host::HostId,
        source: &str,
        target: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self
            .hosts
            .iter_mut()
            .find(|host| host.identity() == *host_id)
        else {
            return;
        };
        let sessions = &mut host.snapshot.sessions;
        let from = sessions.iter().position(|session| session.id == source);
        let to = sessions.iter().position(|session| session.id == target);
        let (Some(from), Some(to)) = (from, to) else {
            return;
        };
        if move_item(sessions, from, to) {
            self.session_order.insert(
                host_id.to_string(),
                sessions
                    .iter()
                    .map(|session| session.order_id.clone())
                    .collect(),
            );
            self.save(cx);
            cx.notify();
        }
    }

    pub(super) fn move_session_relative(
        &mut self,
        host_id: &tshell_host::HostId,
        source: &str,
        delta: isize,
        cx: &mut Context<Self>,
    ) {
        let target = self
            .hosts
            .iter()
            .find(|host| host.identity() == *host_id)
            .and_then(|host| {
                let from = host
                    .snapshot
                    .sessions
                    .iter()
                    .position(|session| session.id == source)?;
                host.snapshot
                    .sessions
                    .get(from.checked_add_signed(delta)?)
                    .map(|session| session.id.clone())
            });
        if let Some(target) = target {
            self.move_session(host_id, source, &target, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Connection, SessionInfo, Snapshot, apply_session_order, move_item, ordered_indices,
        prune_session_order,
    };

    fn session(id: &str, order_id: &str) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            order_id: order_id.into(),
            ..Default::default()
        }
    }

    #[test]
    fn default_order_preserves_creation_and_new_hosts_append_after_custom_order() {
        let keys = vec![
            "local".into(),
            "first".into(),
            "second".into(),
            "new".into(),
        ];
        assert_eq!(ordered_indices(&keys, &[]), vec![0, 1, 2, 3]);
        assert_eq!(
            ordered_indices(
                &keys,
                &[
                    "second".into(),
                    "deleted".into(),
                    "local".into(),
                    "first".into()
                ]
            ),
            vec![2, 0, 1, 3]
        );
    }

    #[test]
    fn session_order_survives_refresh_rename_and_runtime_id_changes() {
        let mut sessions = vec![
            session("new-runtime-first", "first"),
            session("new-runtime-second", "second"),
            session("new-third", "third"),
            session("new-fourth", "fourth"),
        ];
        sessions[1].name = "renamed".into();
        apply_session_order(
            &mut sessions,
            &["second".into(), "deleted".into(), "first".into()],
        );
        assert_eq!(
            sessions
                .iter()
                .map(|session| session.order_id.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "first", "third", "fourth"]
        );
    }

    #[test]
    fn tmux_reused_session_id_does_not_inherit_deleted_session_position() {
        let mut sessions = vec![
            session("$1", "$1:100"),
            session("$2", "$2:200"),
            session("$3", "$3:300"),
        ];
        apply_session_order(
            &mut sessions,
            &["$3:50".into(), "$2:200".into(), "$1:100".into()],
        );
        assert_eq!(
            sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["$2", "$1", "$3"]
        );
    }

    #[test]
    fn moving_both_directions_uses_visible_destination_and_rejects_invalid_targets() {
        let mut items = vec![0, 1, 2];
        assert!(move_item(&mut items, 0, 2));
        assert_eq!(items, vec![1, 2, 0]);
        assert!(move_item(&mut items, 2, 0));
        assert_eq!(items, vec![0, 1, 2]);
        assert!(!move_item(&mut items, 0, 0));
        assert!(!move_item(&mut items, 0, 3));
    }

    #[test]
    fn deleted_sessions_are_pruned_without_reordering_survivors_or_new_sessions() {
        let mut snapshot = Snapshot {
            sessions: vec![
                session("$1", "$1:100"),
                session("$3", "$3:300"),
                session("$4", "$4:400"),
            ],
            connection: Connection::Ready,
            ..Default::default()
        };
        let mut order = vec!["$3:300".into(), "$2:200".into(), "$1:100".into()];
        assert!(prune_session_order(&mut order, &snapshot));
        assert_eq!(order, vec!["$3:300", "$1:100"]);
        apply_session_order(&mut snapshot.sessions, &order);
        assert_eq!(
            snapshot
                .sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["$3", "$1", "$4"]
        );
        assert!(!prune_session_order(&mut order, &snapshot));
        snapshot.sessions.clear();
        assert!(prune_session_order(&mut order, &snapshot));
        assert!(order.is_empty());
    }

    #[test]
    fn connection_gaps_do_not_erase_session_order() {
        for connection in [Connection::Connecting, Connection::Closed] {
            let snapshot = Snapshot {
                connection,
                ..Default::default()
            };
            let mut order = vec!["$1:100".into()];
            assert!(!prune_session_order(&mut order, &snapshot));
            assert_eq!(order, vec!["$1:100"]);
        }
    }
}
