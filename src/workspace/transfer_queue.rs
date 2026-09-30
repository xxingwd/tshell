use super::*;

const MAX_CONCURRENT_TRANSFERS: usize = 2;
const MAX_TRANSFER_ROWS: usize = 32;
const PANEL_WIDTH: f32 = 360.;
const PANEL_HEADER_HEIGHT: f32 = 40.;
const PANEL_FOOTER_HEIGHT: f32 = 32.;
const TRANSFER_ROW_HEIGHT: f32 = 80.;
const PANEL_MARGIN: f32 = 12.;
const PANEL_AUTO_COLLAPSE: Duration = Duration::from_secs(4);

#[cfg(debug_assertions)]
mod ui_check;
#[cfg(debug_assertions)]
pub(super) use ui_check::check_transfer_panel;
#[cfg(test)]
mod tests;

#[derive(Clone)]
enum TransferRequest {
    Download {
        remote: remote_files::Session,
        source: PathBuf,
        destination: PathBuf,
    },
    Upload {
        remote: remote_files::Session,
        sources: Vec<PathBuf>,
        target: PathBuf,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransferState {
    Queued,
    Running,
    Cancelling,
    Done,
    Cancelled,
    Failed,
}

impl TransferState {
    fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled | Self::Failed)
    }
}

struct TransferJob {
    id: u64,
    host: String,
    session: Option<(usize, String)>,
    name: String,
    request: TransferRequest,
    control: remote_files::TransferControl,
    progress: remote_files::TransferProgress,
    state: TransferState,
    error: Option<String>,
}

pub(super) struct TransferQueue {
    jobs: Vec<TransferJob>,
    next_id: u64,
    panel_open: bool,
    panel_revision: u64,
    hovered: bool,
    offset: Point<Pixels>,
    drag: Option<PanelDrag>,
    polling: bool,
}

struct PanelDrag {
    pointer: Point<Pixels>,
    offset: Point<Pixels>,
}

impl Default for TransferQueue {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 0,
            panel_open: false,
            panel_revision: 0,
            hovered: false,
            offset: point(px(PANEL_MARGIN), px(40.)),
            drag: None,
            polling: false,
        }
    }
}

impl TransferQueue {
    pub(super) fn has_pending(&self) -> bool {
        self.jobs.iter().any(|job| !job.state.finished())
    }

    fn panel_size(&self, viewport: Size<Pixels>) -> Size<Pixels> {
        if !self.panel_open {
            return size(px(148.), px(36.));
        }
        let rows = (self.jobs.len() as f32 * TRANSFER_ROW_HEIGHT)
            .min(TRANSFER_ROW_HEIGHT * 4.)
            .min((f32::from(viewport.height) - 124.).max(0.));
        size(
            px(PANEL_WIDTH.min((f32::from(viewport.width) - PANEL_MARGIN * 2.).max(0.))),
            px(PANEL_HEADER_HEIGHT + rows + PANEL_FOOTER_HEIGHT + 2.),
        )
    }

    fn clamped_offset(&self, offset: Point<Pixels>, viewport: Size<Pixels>) -> Point<Pixels> {
        let panel = self.panel_size(viewport);
        point(
            offset.x.clamp(
                px(PANEL_MARGIN),
                (viewport.width - panel.width - px(PANEL_MARGIN)).max(px(PANEL_MARGIN)),
            ),
            offset.y.clamp(
                px(32.),
                (viewport.height - panel.height - px(CHROME_BAR_HEIGHT + PANEL_MARGIN))
                    .max(px(32.)),
            ),
        )
    }

    fn panel_bounds(&self, viewport: Size<Pixels>) -> Bounds<Pixels> {
        let panel = self.panel_size(viewport);
        let mut offset = self.clamped_offset(self.offset, viewport);
        if !self.panel_open {
            offset.x = px(PANEL_MARGIN);
        }
        Bounds::new(
            point(
                viewport.width - panel.width - offset.x,
                viewport.height - panel.height - offset.y,
            ),
            panel,
        )
    }

    fn collapse(&mut self, revision: u64) -> bool {
        if self.panel_open
            && self.panel_revision == revision
            && !self.hovered
            && self.drag.is_none()
        {
            self.panel_open = false;
            self.panel_revision += 1;
            true
        } else {
            false
        }
    }
}

impl Drop for TransferQueue {
    fn drop(&mut self) {
        for job in &self.jobs {
            job.control.cancel();
        }
    }
}

impl AppView {
    pub(super) fn queue_download(
        &mut self,
        remote: remote_files::Session,
        source: PathBuf,
        destination: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.display().to_string());
        self.enqueue_transfer(
            TransferRequest::Download {
                remote,
                source,
                destination,
            },
            name,
            window,
            cx,
        );
    }

    pub(super) fn queue_upload(
        &mut self,
        remote: remote_files::Session,
        sources: Vec<PathBuf>,
        target: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = if sources.len() == 1 {
            sources[0]
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| sources[0].display().to_string())
        } else {
            crate::t!("transfer.files", count = sources.len()).to_string()
        };
        self.enqueue_transfer(
            TransferRequest::Upload {
                remote,
                sources,
                target,
            },
            name,
            window,
            cx,
        );
    }

    fn enqueue_transfer(
        &mut self,
        request: TransferRequest,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let queue = &mut self.transfer_queue;
        if queue.jobs.is_empty() {
            queue.hovered = false;
        }
        if queue.jobs.len() >= MAX_TRANSFER_ROWS {
            if let Some(index) = queue.jobs.iter().position(|job| job.state.finished()) {
                queue.jobs.remove(index);
            }
        }
        if queue.jobs.len() >= MAX_TRANSFER_ROWS {
            window.push_notification(Notification::warning(crate::t!("transfer.queue_full")), cx);
            return;
        }
        queue.next_id += 1;
        queue.jobs.push(TransferJob {
            id: queue.next_id,
            host: self.hosts[self.active].name.clone(),
            session: self.active_file_session.clone(),
            name,
            request,
            control: remote_files::TransferControl::new(),
            progress: Default::default(),
            state: TransferState::Queued,
            error: None,
        });
        self.set_transfer_panel_open(true, window, cx);
        self.pump_transfers(window, cx);
        cx.notify();
    }

    fn pump_transfers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        loop {
            let active = self
                .transfer_queue
                .jobs
                .iter()
                .filter(|job| {
                    matches!(
                        job.state,
                        TransferState::Running | TransferState::Cancelling
                    )
                })
                .count();
            if active >= MAX_CONCURRENT_TRANSFERS {
                break;
            }
            let Some(job) = self
                .transfer_queue
                .jobs
                .iter_mut()
                .find(|job| job.state == TransferState::Queued)
            else {
                break;
            };
            job.state = TransferState::Running;
            let id = job.id;
            let request = job.request.clone();
            let control = job.control.clone();
            cx.spawn_in(window, async move |view, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        match request {
                            TransferRequest::Download {
                                remote,
                                source,
                                destination,
                            } => remote_files::download_with_control(
                                &remote,
                                &source,
                                &destination,
                                control,
                            ),
                            TransferRequest::Upload {
                                remote,
                                sources,
                                target,
                            } => remote_files::upload_with_control(
                                &remote, sources, &target, control,
                            ),
                        }
                    })
                    .await;
                let _ = view.update_in(cx, |app, window, cx| {
                    app.finish_transfer(id, result, window, cx)
                });
            })
            .detach();
        }
        if !self.transfer_queue.polling && self.transfer_queue.has_pending() {
            self.transfer_queue.polling = true;
            cx.spawn_in(window, async move |view, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    let Ok(active) = view.update_in(cx, |app, _, cx| {
                        let mut changed = false;
                        for job in &mut app.transfer_queue.jobs {
                            if matches!(
                                job.state,
                                TransferState::Running | TransferState::Cancelling
                            ) {
                                let progress = job.control.snapshot();
                                if job.progress != progress {
                                    job.progress = progress;
                                    changed = true;
                                }
                            }
                        }
                        if changed {
                            cx.notify();
                        }
                        let active = app.transfer_queue.has_pending();
                        if !active {
                            app.transfer_queue.polling = false;
                        }
                        active
                    }) else {
                        break;
                    };
                    if !active {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    fn finish_transfer(
        &mut self,
        id: u64,
        result: anyhow::Result<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(job) = self.transfer_queue.jobs.iter_mut().find(|job| job.id == id) else {
            return;
        };
        job.progress = job.control.snapshot();
        job.state = match result {
            Ok(()) if !job.control.is_cancelled() => TransferState::Done,
            Ok(()) => TransferState::Cancelled,
            Err(_) if job.control.is_cancelled() => TransferState::Cancelled,
            Err(error) => {
                job.error = Some(format!("{error:#}"));
                TransferState::Failed
            }
        };
        let failed = job.state == TransferState::Failed;
        if job.state == TransferState::Done
            && self.active_file_session == job.session
            && self.workspace_mode == WorkspaceMode::Files
        {
            self.load_files(cx);
        }
        if failed {
            self.set_transfer_panel_open(true, window, cx);
        }
        self.pump_transfers(window, cx);
        cx.notify();
    }

    fn cancel_transfer(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(job) = self.transfer_queue.jobs.iter_mut().find(|job| job.id == id) {
            match job.state {
                TransferState::Queued => job.state = TransferState::Cancelled,
                TransferState::Running => {
                    job.control.cancel();
                    job.state = TransferState::Cancelling;
                }
                _ => return,
            }
        }
        self.pump_transfers(window, cx);
        cx.notify();
    }

    fn clear_transfer(&mut self, id: u64, cx: &mut Context<Self>) {
        self.transfer_queue
            .jobs
            .retain(|job| job.id != id || !job.state.finished());
        cx.notify();
    }

    fn set_transfer_panel_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.transfer_queue.panel_open = open;
        self.transfer_queue.drag = None;
        self.schedule_transfer_collapse(window, cx);
        cx.notify();
    }

    fn schedule_transfer_collapse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let queue = &mut self.transfer_queue;
        queue.panel_revision += 1;
        if !queue.panel_open || queue.hovered || queue.drag.is_some() {
            return;
        }
        let revision = queue.panel_revision;
        cx.spawn_in(window, async move |view, cx| {
            cx.background_executor().timer(PANEL_AUTO_COLLAPSE).await;
            let _ = view.update_in(cx, |app, _, cx| {
                if app.transfer_queue.collapse(revision) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn move_transfer_panel(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let queue = &mut self.transfer_queue;
        let Some(drag) = &queue.drag else { return };
        if !event.dragging() {
            self.end_transfer_panel_drag(window, cx);
            return;
        }
        let delta = event.position - drag.pointer;
        if delta.x.abs() < px(4.) && delta.y.abs() < px(4.) {
            return;
        }
        queue.offset = queue.clamped_offset(drag.offset - delta, window.viewport_size());
        cx.notify();
    }

    pub(super) fn end_transfer_panel_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.transfer_queue.drag.take().is_some() {
            self.schedule_transfer_collapse(window, cx);
            cx.notify();
        }
    }

    pub(super) fn transfer_panel(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let queue = &self.transfer_queue;
        if queue.jobs.is_empty() {
            return None;
        }
        let p = self.palette;
        let pending = queue
            .jobs
            .iter()
            .filter(|job| !job.state.finished())
            .count();
        let failed = queue
            .jobs
            .iter()
            .filter(|job| job.state == TransferState::Failed)
            .count();
        let compact_summary = if pending > 0 {
            crate::t!("transfer.active_count", count = pending).to_string()
        } else if failed > 0 {
            crate::t!("transfer.failed_count", count = failed).to_string()
        } else {
            crate::t!("transfer.complete").to_string()
        };
        let summary = if pending > 0 && failed > 0 {
            crate::t!("transfer.summary_count", pending = pending, failed = failed).to_string()
        } else {
            compact_summary.clone()
        };
        let expanded = queue.panel_open;
        let bounds = queue.panel_bounds(window.viewport_size());
        let mut panel = div()
            .id("transfer-panel")
            .occlude()
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .flex()
            .flex_col()
            .bg(rgb(p.panel))
            .border_1()
            .border_color(rgb(p.border))
            .rounded(px(6.))
            .shadow_md()
            .on_hover(cx.listener(|app, hovered: &bool, window, cx| {
                app.transfer_queue.hovered = *hovered;
                app.schedule_transfer_collapse(window, cx);
            }));
        if !expanded {
            return Some(
                panel
                    .child(
                        Button::new("transfer-expand")
                            .ghost()
                            .w_full()
                            .h_full()
                            .px_2()
                            .icon(if failed > 0 {
                                IconName::CircleAlert
                            } else {
                                IconName::ArrowDownUp
                            })
                            .label(compact_summary)
                            .text_size(px(12.))
                            .text_color(rgb(if failed > 0 { p.error } else { p.text }))
                            .tooltip(format!("{}: {}", crate::t!("transfer.expand"), summary))
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.set_transfer_panel_open(true, window, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        let header = div()
            .h(px(PANEL_HEADER_HEIGHT))
            .flex_shrink_0()
            .px(px(6.))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .id("transfer-drag")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor(if queue.drag.is_some() {
                        CursorStyle::ClosedHand
                    } else {
                        CursorStyle::OpenHand
                    })
                    .child(
                        Icon::new(IconName::GripVertical)
                            .size(px(14.))
                            .text_color(rgb(p.muted)),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::t!("transfer.title")),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(p.muted))
                            .child(queue.jobs.len().to_string()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|app, event: &MouseDownEvent, window, cx| {
                            let queue = &mut app.transfer_queue;
                            queue.offset =
                                queue.clamped_offset(queue.offset, window.viewport_size());
                            queue.drag = Some(PanelDrag {
                                pointer: event.position,
                                offset: queue.offset,
                            });
                            queue.panel_revision += 1;
                            window.prevent_default();
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .child(
                icon_button(
                    "transfer-dock",
                    IconName::PanelRight,
                    crate::t!("transfer.dock"),
                )
                .w(px(28.))
                .h(px(28.))
                .on_click(cx.listener(|app, _, window, cx| {
                    app.transfer_queue.offset = point(px(PANEL_MARGIN), px(40.));
                    app.schedule_transfer_collapse(window, cx);
                    cx.notify();
                })),
            )
            .child(
                icon_button(
                    "transfer-toggle",
                    IconName::Minus,
                    crate::t!("transfer.collapse"),
                )
                .w(px(28.))
                .h(px(28.))
                .on_click(cx.listener(|app, _, window, cx| {
                    app.set_transfer_panel_open(false, window, cx);
                })),
            );
        panel = panel.child(header);
        let mut rows = div()
            .id("transfer-rows")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar();
        for job in queue.jobs.iter().rev() {
            let status = match job.state {
                TransferState::Queued => crate::t!("transfer.queued").to_string(),
                TransferState::Running if job.progress.scanning => {
                    crate::t!("transfer.scanning").to_string()
                }
                TransferState::Running => crate::t!("transfer.running").to_string(),
                TransferState::Cancelling => crate::t!("transfer.cancelling").to_string(),
                TransferState::Done => crate::t!("transfer.done").to_string(),
                TransferState::Cancelled => crate::t!("transfer.cancelled").to_string(),
                TransferState::Failed => crate::t!("transfer.failed").to_string(),
            };
            let direction = match &job.request {
                TransferRequest::Download { .. } => IconName::Download,
                TransferRequest::Upload { .. } => IconName::Upload,
            };
            let percent = job
                .progress
                .total
                .filter(|total| *total > 0)
                .map(|total| (job.progress.transferred as f32 / total as f32).clamp(0., 1.));
            let bytes = if let Some(total) = job.progress.total {
                format!(
                    "{} / {}",
                    transfer_bytes(job.progress.transferred),
                    transfer_bytes(total)
                )
            } else {
                transfer_bytes(job.progress.transferred)
            };
            let detail = job.error.clone().unwrap_or_else(|| {
                if job.progress.current_file.is_empty() {
                    job.host.clone()
                } else {
                    format!("{} / {}", job.host, job.progress.current_file)
                }
            });
            let status_color = if job.state == TransferState::Failed {
                p.error
            } else if job.state == TransferState::Running {
                p.accent
            } else {
                p.muted
            };
            let id = job.id;
            let mut row = div()
                .id(("transfer-row", id))
                .h(px(TRANSFER_ROW_HEIGHT))
                .flex_shrink_0()
                .px(px(12.))
                .py(px(8.))
                .border_b_1()
                .border_color(rgb(p.border))
                .flex()
                .items_start()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(16.))
                        .pt(px(3.))
                        .flex_shrink_0()
                        .child(Icon::new(direction).size_4().text_color(rgb(p.muted))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .id(("transfer-name", id))
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(13.))
                                        .child(job.name.clone())
                                        .tooltip({
                                            let name = job.name.clone();
                                            move |window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(
                                                    name.clone(),
                                                )
                                                .build(window, cx)
                                            }
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(px(12.))
                                        .text_color(rgb(status_color))
                                        .child(status),
                                ),
                        )
                        .child(
                            div()
                                .id(("transfer-detail", id))
                                .truncate()
                                .text_size(px(12.))
                                .text_color(rgb(if job.state == TransferState::Failed {
                                    p.error
                                } else {
                                    p.muted
                                }))
                                .child(detail.clone())
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(detail.clone())
                                        .build(window, cx)
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .h(px(3.))
                                        .rounded(px(2.))
                                        .bg(rgb(p.border))
                                        .when_some(percent, |bar, percent| {
                                            bar.child(
                                                div()
                                                    .h_full()
                                                    .w(relative(percent))
                                                    .rounded(px(2.))
                                                    .bg(rgb(status_color)),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(px(11.))
                                        .text_color(rgb(p.muted))
                                        .child(bytes),
                                ),
                        ),
                );
            if matches!(job.state, TransferState::Queued | TransferState::Running) {
                row = row.child(
                    icon_button(
                        format!("transfer-cancel-{id}"),
                        IconName::X,
                        crate::t!("transfer.cancel"),
                    )
                    .w(px(26.))
                    .h(px(26.))
                    .flex_shrink_0()
                    .on_click(
                        cx.listener(move |app, _, window, cx| app.cancel_transfer(id, window, cx)),
                    ),
                );
            } else if job.state.finished() {
                row = row.child(
                    icon_button(
                        format!("transfer-clear-{id}"),
                        IconName::Trash,
                        crate::t!("transfer.clear_item"),
                    )
                    .w(px(26.))
                    .h(px(26.))
                    .flex_shrink_0()
                    .on_click(cx.listener(move |app, _, _, cx| app.clear_transfer(id, cx))),
                );
            } else {
                row = row.child(div().w(px(26.)).h(px(26.)).flex_shrink_0());
            }
            rows = rows.child(row);
        }
        panel = panel.child(rows).child(
            div()
                .h(px(PANEL_FOOTER_HEIGHT))
                .flex_shrink_0()
                .px_2()
                .border_t_1()
                .border_color(rgb(p.border))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(if failed > 0 { p.error } else { p.muted }))
                        .child(summary),
                )
                .child(
                    Button::new("transfer-clear")
                        .ghost()
                        .small()
                        .text_size(px(12.))
                        .disabled(!queue.jobs.iter().any(|job| job.state.finished()))
                        .label(crate::t!("transfer.clear"))
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.transfer_queue.jobs.retain(|job| !job.state.finished());
                            cx.notify();
                        })),
                ),
        );
        Some(panel.into_any_element())
    }
}

fn transfer_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64;
    let mut unit = "KiB";
    for next in ["KiB", "MiB", "GiB", "TiB"] {
        size /= 1024.;
        unit = next;
        if size < 1024. {
            break;
        }
    }
    format!("{size:.1} {unit}")
}
