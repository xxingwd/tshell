use super::*;

#[derive(Clone)]
pub(super) struct SessionTarget {
    id: String,
    name: String,
    path: String,
}

#[derive(Clone)]
pub(super) enum PaletteTarget {
    NewSession,
    Shortcut(Shortcut),
    Host(usize),
    Session(SessionTarget),
}

struct PaletteGroup {
    label: &'static str,
    targets: Vec<PaletteTarget>,
}

impl AppView {
    fn palette_groups(&self) -> [PaletteGroup; 3] {
        let common = [
            Shortcut::NewWindow,
            Shortcut::ShowTerminal,
            Shortcut::ShowFiles,
            Shortcut::ShowGit,
            Shortcut::Settings,
        ];
        let mut operations = vec![PaletteTarget::NewSession];
        operations.extend(common.into_iter().map(PaletteTarget::Shortcut));
        operations.extend(
            shortcuts::BINDINGS
                .iter()
                .filter(|binding| {
                    !common.contains(&binding.action)
                        && !matches!(
                            binding.action,
                            Shortcut::CommandPalette
                                | Shortcut::Session(_)
                                | Shortcut::SelectSession(_)
                        )
                })
                .map(|binding| PaletteTarget::Shortcut(binding.action)),
        );
        [
            PaletteGroup {
                label: "ws.group_hosts",
                targets: (0..self.hosts.len()).map(PaletteTarget::Host).collect(),
            },
            PaletteGroup {
                label: "ws.group_sessions",
                targets: self
                    .launcher_sessions()
                    .into_iter()
                    .map(PaletteTarget::Session)
                    .collect(),
            },
            PaletteGroup {
                label: "ws.group_operations",
                targets: operations,
            },
        ]
    }

    fn palette_shortcut_item(&self, action: Shortcut) -> CommandItem {
        let Some(binding) = shortcuts::BINDINGS
            .iter()
            .find(|binding| binding.action == action)
        else {
            return CommandItem::new().disabled(true);
        };
        let snapshot = &self.hosts[self.active].snapshot;
        let disabled = match action {
            Shortcut::SaveFile => self.open_file.is_none(),
            Shortcut::RemoveHost => self.active == 0,
            Shortcut::PreviousHost | Shortcut::NextHost => self.hosts.len() < 2,
            Shortcut::TerminalSearch
            | Shortcut::CloseWindow
            | Shortcut::ClosePane
            | Shortcut::Focus
            | Shortcut::Floating
            | Shortcut::NewColumn
            | Shortcut::NewRow
            | Shortcut::Move(_)
            | Shortcut::MovePane(_) => snapshot.window().is_none(),
            _ => false,
        };
        let label = binding.label();
        let key = shortcuts::keys(binding, &self.keybindings)
            .first()
            .and_then(|key| Keystroke::parse(key).ok());
        let icon = command_icon(action);
        CommandItem::new()
            .label(label.clone())
            .keywords([binding.id])
            .icon(icon.clone())
            .disabled(disabled)
            .child(move |_, _| {
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(icon.clone()).small().flex_shrink_0())
                    .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                    .when_some(key.clone(), |row, key| row.child(Kbd::new(key).outline()))
            })
    }

    fn palette_target_item(&self, target: &PaletteTarget) -> CommandItem {
        match target {
            PaletteTarget::NewSession => CommandItem::new()
                .label(crate::t!("ws.new_session"))
                .icon(IconName::Plus)
                .keywords(["new_session"]),
            PaletteTarget::Shortcut(action) => self.palette_shortcut_item(*action),
            PaletteTarget::Host(index) => {
                let host = &self.hosts[*index];
                let destination = host
                    .config
                    .as_ref()
                    .map(|config| config.destination.clone())
                    .unwrap_or_else(|| crate::t!("ws.local_keywords").to_string());
                CommandItem::new()
                    .label(host.name.clone())
                    .icon(IconName::Server)
                    .keywords([host.name.clone(), destination])
                    .checked(*index == self.active)
            }
            PaletteTarget::Session(session) => self.launcher_session_item(session),
        }
    }

    pub(super) fn run_palette_target(
        &mut self,
        target: PaletteTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            PaletteTarget::Session(session) => self.launch_session(session, window, cx),
            PaletteTarget::NewSession => {
                self.close_modal(window, cx);
                self.show_new_session(window, cx);
            }
            PaletteTarget::Shortcut(action) => {
                self.close_modal(window, cx);
                self.execute_shortcut(action, window, cx);
            }
            PaletteTarget::Host(host) => {
                if host < self.hosts.len() {
                    self.close_modal(window, cx);
                    self.switch_host(host, cx);
                    self.show_terminal(cx);
                }
            }
        }
    }

    pub(super) fn command_view(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let groups = self.palette_groups();
        let height = 410_f32.min((f32::from(window.viewport_size().height) - 180.).max(180.));
        let owner = cx.entity().downgrade();
        let mut command = Command::new(&self.command_state);
        for group in &groups {
            command = command.group(
                CommandGroup::new()
                    .label(crate::t!(group.label).into_owned())
                    .items(
                        group
                            .targets
                            .iter()
                            .map(|target| self.palette_target_item(target)),
                    ),
            );
        }
        command
            .placeholder(crate::t!("ws.command_search"))
            .bordered(false)
            .max_h(px((height - 44.).max(50.)))
            .empty(|_, _, _| div().p_3().child(crate::t!("ws.palette_empty")))
            .on_confirm(move |index, window, cx| {
                if let Some(target) = groups
                    .get(index.section)
                    .and_then(|group| group.targets.get(index.row))
                {
                    let _ = owner.update(cx, |app, cx| {
                        app.run_palette_target(target.clone(), window, cx)
                    });
                }
            })
            .into_any_element()
    }

    pub(super) fn launcher_sessions(&self) -> Vec<SessionTarget> {
        self.hosts[self.active]
            .snapshot
            .sessions
            .iter()
            .map(|session| SessionTarget {
                id: session.id.clone(),
                name: session.name.clone(),
                path: session.cwd.clone(),
            })
            .collect()
    }

    fn is_current_session(&self, target: &SessionTarget) -> bool {
        target.id == self.hosts[self.active].snapshot.active_session
    }

    pub(super) fn launcher_session_item(&self, target: &SessionTarget) -> CommandItem {
        CommandItem::new()
            .label(target.name.clone())
            .icon(IconName::Terminal)
            .checked(self.is_current_session(target))
            .keywords([target.name.clone(), target.path.clone()])
    }

    pub(super) fn launch_session(
        &mut self,
        target: SessionTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_modal(window, cx);
        let host = &self.hosts[self.active];
        let id = target.id;
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
            self.hosts[self.active]
                .pending
                .push(Action::SelectSession(id));
        }
        self.show_terminal(cx);
    }
}
