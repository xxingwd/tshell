extern crate gpui_kit as gpui;
use gpui_kit::assets::IconName;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::{
    Icon, Selectable, Sizable,
    button::{Button, ButtonGroup, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{prelude::*, *};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tshell_ui::appearance::Palette;
use tshell_ui::controls::{self as style, icon_button};
pub use tshell_ui::{i18n, t};
pub mod git;
pub mod selection;
pub mod view;
pub use view::{DiffMode, DiffView};
#[derive(Clone)]
pub enum Event {
    OpenFile(PathBuf),
}
pub struct GitView {
    _auxiliary: Option<tshell_host::ssh::Auxiliary>,
    pub host: Option<tshell_host::SshConfig>,
    pub cwd: PathBuf,
    pub git_request: u64,
    pub git_diff_request: u64,
    pub git_diff: Option<DiffView>,
    pub git_diff_mode: DiffMode,
    pub git_diff_compact: bool,
    pub git_changes: Vec<git::GitChange>,
    pub git_scroll: UniformListScrollHandle,
    pub git_error: Option<String>,
    pub git_loading: bool,
    pub diff_loading: bool,
    pub palette: Palette,
    pub terminal_palette: Palette,
    pub font_family: String,
    pub font_size: f32,
}
impl EventEmitter<Event> for GitView {}
impl GitView {
    pub fn new(
        host: Option<tshell_host::SshConfig>,
        cwd: PathBuf,
        palette: Palette,
        terminal_palette: Palette,
        font_family: String,
        font_size: f32,
    ) -> Self {
        let auxiliary = host.as_ref().map(tshell_host::ssh::auxiliary);
        Self {
            _auxiliary: auxiliary,
            host,
            cwd,
            palette,
            terminal_palette,
            font_family,
            font_size,
            git_request: 0,
            git_diff_request: 0,
            git_diff: None,
            git_diff_mode: DiffMode::SideBySide,
            git_diff_compact: true,
            git_changes: Vec::new(),
            git_scroll: UniformListScrollHandle::new(),
            git_error: None,
            git_loading: false,
            diff_loading: false,
        }
    }
    pub fn refresh_git(&mut self, cx: &mut Context<Self>) {
        self.git_request += 1;
        let request = self.git_request;
        let root = self.cwd.clone();
        let host = self.host.clone();
        self.git_loading = true;
        self.git_error = None;
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
                    this.git_loading = false;
                    match result {
                        Ok(changes) => {
                            if let Some(selected) = this.git_diff.as_ref() {
                                let selected_path = selected.change.path.clone();
                                if let Some(change) =
                                    changes.iter().find(|change| change.path == selected_path)
                                {
                                    this.load_git_diff(change.clone(), false, cx);
                                } else {
                                    this.git_diff = None;
                                    this.diff_loading = false;
                                    this.git_diff_request += 1;
                                }
                            }
                            this.git_changes = changes.clone();
                            this.git_error = None;
                            cx.notify();
                            Some(changes)
                        }
                        Err(error) => {
                            this.git_error = Some(crate::i18n::format_error(&error));
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
                            crate::t!(
                                "tools.line_count_failed",
                                error = crate::i18n::format_error(&error)
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
                                    error = crate::i18n::format_error(&error)
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
impl Render for GitView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.git_surface(cx)
    }
}
