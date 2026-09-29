mod selection_scroll;
use crate::{
    appearance::Palette,
    input::{MouseReport, encode_focus, encode_key, encode_mouse, mouse_reporting},
    terminal::Session,
    terminal_render::{
        PaintSpan, build_row, expand_wide_selection, hit_test, resolve_color, selection_spans,
    },
    terminal_snapshot::{Anchor, RenderCommand, RenderCursor, RenderMailbox, RenderSnapshot},
};
use alacritty_terminal::{
    grid::Scroll,
    selection::SelectionType,
    term::{
        TermMode,
        cell::{Cell, Flags},
        color::{COUNT, Colors},
    },
    vte::ansi::{Color, CursorShape, NamedColor},
};
use gpui_kit::component::alert::Alert;
use gpui_kit::{prelude::*, *};
use std::{
    ops::Range,
    rc::Rc,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

mod row_cache;
use row_cache::{RowTextView, ascii_grid_aligned, ligature_grid_aligned};

const KEY_CONTEXT: &str = "Terminal";
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(600);
const CURSOR_BEAM_WIDTH: f32 = 1.;

fn cursor_geometry(
    origin: Point<Pixels>,
    shape: CursorShape,
    cell: Option<&Cell>,
    cell_width: f32,
    line_height: f32,
) -> (Point<Pixels>, Size<Pixels>) {
    let width = cell_width
        * if cell.is_some_and(|cell| cell.flags.contains(Flags::WIDE_CHAR)) {
            2.
        } else {
            1.
        };
    match shape {
        CursorShape::Underline => (
            origin + point(px(0.), px(line_height - 2.)),
            size(px(width), px(2.)),
        ),
        CursorShape::Beam => (origin, size(px(CURSOR_BEAM_WIDTH), px(line_height))),
        _ => (origin, size(px(width), px(line_height))),
    }
}

fn cursor_should_paint(focused: bool, blink_visible: bool) -> bool {
    focused && blink_visible
}

pub(crate) fn init(cx: &mut App) {
    crate::terminal_clipboard::init(cx);
    // Root binds these keys to focus traversal. Disable those actions while a
    // terminal is focused so the low-level handler can send the xterm bytes.
    cx.bind_keys([
        KeyBinding::new("tab", NoAction, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", NoAction, Some(KEY_CONTEXT)),
    ]);
}

struct PaintedText {
    col: usize,
    #[cfg(debug_assertions)]
    columns: usize,
    shaped: ShapedLine,
}
struct PaintedRow {
    source: Arc<[Cell]>,
    text: Vec<PaintedText>,
    glyphs: Vec<crate::terminal_glyphs::GlyphCell>,
    decorations: Vec<crate::terminal_decorations::Decoration>,
    backgrounds: Vec<PaintSpan>,
}
struct Frame {
    rows: Vec<Arc<PaintedRow>>,
    text_elements: Vec<AnyElement>,
    selection: Vec<PaintSpan>,
    cursor: Option<RenderCursor>,
    background: u32,
}

pub struct TerminalView {
    pub session: Option<Arc<Session>>,
    pub failure: Option<String>,
    pub focus: FocusHandle,
    pub font_size: f32,
    pub line_height_scale: crate::terminal_metrics::LineHeight,
    pub font_family: String,
    pub ligatures: bool,
    pub background_inherited: bool,
    pub palette: Palette,
    pub revision: u64,
    bounds: Bounds<Pixels>,
    cell_width: f32,
    line_height: f32,
    measured_font_size: u32,
    measured_font_family: String,
    painted_ligatures: bool,
    selecting: bool,
    drag_position: Option<Point<Pixels>>,
    drag_scroll_task: Option<Task<()>>,
    block_selection: bool,
    selection_point: Option<Anchor>,
    pending_anchor: Option<Anchor>,
    visible: bool,
    last_mouse_cell: Option<(usize, usize)>,
    reported_button: Option<MouseButton>,
    scroll_pixels: f32,
    preedit: String,
    preedit_selection: Range<usize>,
    cursor: (usize, usize),
    snapshot: Arc<RenderSnapshot>,
    snapshot_in_flight: bool,
    snapshot_pending: bool,
    interactions_in_flight: bool,
    interaction_task: Option<Task<()>>,
    mailbox: RenderMailbox,
    snapshot_task: Option<Task<()>>,
    painted_rows: Vec<Arc<PaintedRow>>,
    row_views: Vec<Option<Entity<RowTextView>>>,
    row_paints: Rc<std::cell::Cell<usize>>,
    #[cfg(debug_assertions)]
    cache_row_paint: bool,
    painted_palette: Palette,
    painted_colors: Colors,
    focus_subscriptions: Vec<Subscription>,
    cursor_focused: bool,
    cursor_visible: bool,
    cursor_blinking: bool,
    cursor_blink_task: Option<Task<()>>,
    _output_task: Task<()>,
}

impl TerminalView {
    fn session_mode(&self) -> TermMode {
        self.session.as_ref().map(|s| s.mode()).unwrap_or_default()
    }

    pub fn from_session(
        session: Arc<Session>,
        font_size: f32,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Self {
        let updates = session.updates.subscribe();
        let output_task = cx.spawn(async move |view, cx| {
            while updates.recv().await.is_ok() {
                while updates.try_recv().is_ok() {}
                if view.update(cx, |v, cx| v.changed(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            session: Some(session),
            failure: None,
            focus: cx.focus_handle(),
            font_size,
            line_height_scale: Default::default(),
            font_family: "Consolas".into(),
            ligatures: false,
            background_inherited: false,
            palette,
            revision: 0,
            bounds: Bounds::default(),
            cell_width: font_size * 0.6,
            line_height: font_size,
            measured_font_size: 0,
            measured_font_family: String::new(),
            painted_ligatures: false,
            selecting: false,
            drag_position: None,
            drag_scroll_task: None,
            block_selection: false,
            selection_point: None,
            pending_anchor: None,
            visible: false,
            last_mouse_cell: None,
            reported_button: None,
            scroll_pixels: 0.,
            preedit: String::new(),
            preedit_selection: 0..0,
            cursor: (0, 0),
            snapshot: Arc::new(RenderSnapshot::default()),
            snapshot_in_flight: false,
            snapshot_pending: false,
            interactions_in_flight: false,
            interaction_task: None,
            mailbox: RenderMailbox::default(),
            snapshot_task: None,
            painted_rows: Vec::new(),
            row_views: Vec::new(),
            row_paints: Rc::new(std::cell::Cell::new(0)),
            #[cfg(debug_assertions)]
            cache_row_paint: true,
            painted_palette: palette,
            painted_colors: Colors::default(),
            focus_subscriptions: Vec::new(),
            cursor_focused: false,
            cursor_visible: false,
            cursor_blinking: false,
            cursor_blink_task: None,
            _output_task: output_task,
        }
    }

    pub fn changed(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(session) = &self.session {
            let failure = session.error.lock().clone();
            if failure != self.failure {
                self.failure = failure;
                cx.notify();
            }
            let revision = session.revision.load(Ordering::Acquire);
            if revision != self.revision {
                self.revision = revision;
                self.request_snapshot(cx);
                return true;
            }
        }
        false
    }

    fn apply_snapshot(&mut self, snapshot: Arc<RenderSnapshot>, cx: &mut Context<Self>) -> bool {
        let visual_changed = !self.snapshot.same_visual(&snapshot);
        let cursor_changed = self.snapshot.cursor != snapshot.cursor;
        self.snapshot = snapshot;
        if cursor_changed {
            self.cursor_blinking = self.snapshot.cursor.is_some_and(|cursor| cursor.blinking);
            self.restart_cursor_blink(cx);
        }
        visual_changed
    }

    // At most one snapshot job per view. Intermediate output notifications are
    // replaced by the latest state; no task-per-mouse-event or unbounded queue.
    fn request_snapshot(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        if self.snapshot_in_flight {
            self.snapshot_pending = true;
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        self.snapshot_in_flight = true;
        self.snapshot_pending = false;
        self.snapshot_task = Some(cx.spawn(async move |view, cx| {
            loop {
                let source = session.clone();
                let (snapshot, _) = cx
                    .background_executor()
                    .spawn(async move { source.render_snapshot(Default::default()) })
                    .await;
                let next = view.update(cx, |this, cx| {
                    let visual_changed = this.apply_snapshot(snapshot, cx);
                    if visual_changed {
                        cx.notify();
                    }
                    if !this.visible
                        || (!this.snapshot_pending
                            && session.revision.load(Ordering::Acquire) == this.snapshot.revision)
                    {
                        this.snapshot_in_flight = false;
                        false
                    } else {
                        this.snapshot_pending = false;
                        true
                    }
                });
                match next {
                    Ok(true) => {}
                    _ => break,
                }
            }
        }));
    }

    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible != visible {
            self.visible = visible;
            if !visible {
                self.finish_selection(cx);
            }
            if visible {
                self.request_snapshot(cx);
            }
        }
    }

    fn queue_render(&mut self, command: RenderCommand, cx: &mut Context<Self>) -> bool {
        if !self.flush_selection(cx) {
            return false;
        }
        self.submit_command(command, cx)
    }

    fn submit_command(&mut self, command: RenderCommand, cx: &mut Context<Self>) -> bool {
        if !self.mailbox.push(command) {
            self.failure = Some(crate::t!("term.interaction_queue_full").to_string().into());
            cx.notify();
            return false;
        }
        if self.interactions_in_flight {
            return true;
        }
        let Some(session) = self.session.clone() else {
            return true;
        };
        self.interactions_in_flight = true;
        let mut commands = self.mailbox.take();
        self.interaction_task = Some(cx.spawn(async move |view, cx| {
            loop {
                let source = session.clone();
                let copies = cx
                    .background_executor()
                    .spawn(async move { source.apply_interactions(commands) })
                    .await;
                let next = view.update(cx, |this, cx| {
                    for text in copies {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    this.request_snapshot(cx);
                    let queued = this.mailbox.take();
                    this.flush_selection(cx);
                    if queued.is_empty() && this.mailbox.is_empty() {
                        this.interactions_in_flight = false;
                        None
                    } else if queued.is_empty() {
                        Some(this.mailbox.take())
                    } else {
                        Some(queued)
                    }
                });
                match next {
                    Ok(Some(next)) => commands = next,
                    _ => break,
                }
            }
        }));
        true
    }

    fn flush_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(anchor) = self.pending_anchor else {
            return true;
        };
        if self.submit_command(RenderCommand::Extend(anchor), cx) {
            self.pending_anchor = None;
            true
        } else {
            false
        }
    }

    fn finish_selection(&mut self, cx: &mut Context<Self>) {
        self.drag_scroll_task = None;
        self.drag_position = None;
        if self.selecting {
            // Like xterm's mouseup, keep the last move/scroll endpoint. A hit
            // test here would truncate an outside drag or use a stale viewport.
            self.flush_selection(cx);
        }
        self.selecting = false;
        self.selection_point = None;
    }

    fn move_selection(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting {
            let mode = self.session_mode();
            if event.modifiers.shift || !mouse_reporting(mode) {
                return;
            }
            let cell = self.viewport_cell(event.position);
            if self.last_mouse_cell == Some(cell) {
                return;
            }
            if let Some(bytes) = encode_mouse(
                MouseReport::Motion(event.pressed_button),
                cell.0,
                cell.1,
                &event.modifiers,
                mode,
            ) {
                if self.queue_render(RenderCommand::MouseMove(bytes), cx) {
                    self.last_mouse_cell = Some(cell);
                }
            }
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.finish_selection(cx);
            return;
        }
        self.update_drag_scroll(event.position, cx);
        let anchor = self.grid_anchor(event.position);
        if self.selection_point == Some(anchor) {
            return;
        }
        self.selection_point = Some(anchor);
        self.pending_anchor = Some(anchor);
        self.flush_selection(cx);
    }

    pub fn send(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.finish_selection(cx);
        self.queue_render(RenderCommand::Input(bytes), cx);
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        self.flush_selection(cx);
        self.queue_render(RenderCommand::Copy, cx);
    }

    pub fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            let mode = self.session_mode();
            let bytes = crate::input::encode_paste(&text, mode);
            self.send(bytes, cx);
        }
    }

    fn grid_anchor(&self, position: Point<Pixels>) -> Anchor {
        hit_test(
            f32::from(position.x - self.bounds.left()),
            f32::from(position.y - self.bounds.top()),
            self.cell_width,
            self.line_height,
            self.snapshot.rows.len(),
            self.snapshot.cols,
            self.snapshot.display_offset,
        )
    }

    fn viewport_cell(&self, position: Point<Pixels>) -> (usize, usize) {
        let anchor = self.grid_anchor(position);
        (
            (anchor.point.line.0 + self.snapshot.display_offset as i32).max(0) as usize,
            anchor.point.column.0,
        )
    }

    fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        window.focus(&self.focus, cx);
        self.finish_selection(cx);
        let mode = self.session_mode();

        if event.button == MouseButton::Left && (event.modifiers.shift || !mouse_reporting(mode)) {
            let anchor = self.grid_anchor(event.position);
            let ty = if event.modifiers.alt {
                SelectionType::Block
            } else {
                match event.click_count {
                    2 => SelectionType::Semantic,
                    3.. => SelectionType::Lines,
                    _ => SelectionType::Simple,
                }
            };
            if self.queue_render(RenderCommand::Begin(anchor, ty), cx) {
                self.selecting = true;
                self.block_selection = event.modifiers.alt;
                self.selection_point = Some(anchor);
            }
            return false;
        }
        if event.modifiers.shift {
            return false;
        }

        let cell = self.viewport_cell(event.position);
        let Some(bytes) = encode_mouse(
            MouseReport::Press(event.button),
            cell.0,
            cell.1,
            &event.modifiers,
            mode,
        ) else {
            return false;
        };
        if self.queue_render(RenderCommand::Input(bytes), cx) {
            self.last_mouse_cell = Some(cell);
            self.reported_button = Some(event.button);
            window.prevent_default();
            true
        } else {
            false
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) -> bool {
        if self.selecting {
            self.finish_selection(cx);
            return false;
        }
        if self.reported_button != Some(event.button) {
            return false;
        }
        let mode = self.session_mode();
        let cell = self.viewport_cell(event.position);
        let Some(bytes) = encode_mouse(
            MouseReport::Release(event.button),
            cell.0,
            cell.1,
            &event.modifiers,
            mode,
        ) else {
            return false;
        };
        if self.queue_render(RenderCommand::Input(bytes), cx) {
            self.last_mouse_cell = Some(cell);
            self.reported_button = None;
            true
        } else {
            false
        }
    }

    fn handle_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mouse_up(event, cx) {
            window.prevent_default();
            if event.button != MouseButton::Left {
                cx.stop_propagation();
            }
        }
    }

    fn restart_cursor_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_blink_task = None;
        self.cursor_visible = self.cursor_focused;
        if !self.cursor_focused || !self.cursor_blinking {
            cx.notify();
            return;
        }
        self.cursor_blink_task = Some(cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
                let keep_blinking = view
                    .update(cx, |this, cx| {
                        if !this.cursor_focused || !this.cursor_blinking {
                            return false;
                        }
                        this.cursor_visible = !this.cursor_visible;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_blinking {
                    break;
                }
            }
        }));
    }

    fn focus_changed(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.cursor_focused == focused {
            return;
        }
        if !focused {
            self.finish_selection(cx);
        }
        self.cursor_focused = focused;
        self.restart_cursor_blink(cx);
        let mode = self.session_mode();
        if let Some(bytes) = encode_focus(focused, mode) {
            self.queue_render(RenderCommand::Input(bytes), cx);
        }
    }

    fn terminal_font(&self) -> Font {
        let text_font = font(self.font_family.clone());
        #[cfg(windows)]
        {
            // Chromium's Windows simplified-Han fallback order. Keep the
            // selected primary font and system fallback for other scripts.
            static FALLBACKS: std::sync::OnceLock<FontFallbacks> = std::sync::OnceLock::new();
            Font {
                fallbacks: Some(
                    FALLBACKS
                        .get_or_init(|| {
                            FontFallbacks::from_fonts(
                                [
                                    "Noto Sans SC",
                                    "Noto Sans CJK SC",
                                    "Microsoft YaHei",
                                    "SimSun",
                                ]
                                .into_iter()
                                .map(str::to_owned)
                                .collect(),
                            )
                        })
                        .clone(),
                ),
                ..text_font
            }
        }
        #[cfg(not(windows))]
        text_font
    }

    fn text_run(&self, text: &str, fg: u32, flags: Flags) -> TextRun {
        let mut text_font = self.terminal_font();
        text_font.features = FontFeatures(Arc::new(vec![
            ("calt".into(), u32::from(self.ligatures)),
            ("liga".into(), u32::from(self.ligatures)),
            ("clig".into(), u32::from(self.ligatures)),
            ("kern".into(), 0),
        ]));
        if flags.contains(Flags::BOLD) {
            text_font.weight = FontWeight::BOLD;
        }
        if flags.contains(Flags::ITALIC) {
            text_font.style = FontStyle::Italic;
        }
        TextRun {
            len: text.len(),
            font: text_font,
            color: crate::terminal_decorations::ink(fg, flags.contains(Flags::DIM)),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    fn shape_row(&self, source: Arc<[Cell]>, window: &mut Window) -> PaintedRow {
        let plan = build_row(&source, self.palette, &self.snapshot.colors);
        let mut text = Vec::with_capacity(plan.text.len());
        for run in plan.text {
            let style = self.text_run(&run.text, run.fg, run.flags);
            let shaped = window.text_system().shape_line(
                run.text.clone().into(),
                px(self.font_size),
                &[style],
                (run.ascii && !self.ligatures).then_some(px(self.cell_width)),
            );
            // GPUI's forced advance has a tolerance and can still shape ligatures.
            // Validate EVERY ASCII cell origin. On mismatch isolate cells so font
            // fallback and shaping can never shift subsequent terminal columns.
            let aligned = !run.ascii
                || if self.ligatures {
                    ligature_grid_aligned(&shaped, run.columns, self.cell_width)
                } else {
                    ascii_grid_aligned(&shaped, run.columns, self.cell_width)
                };
            if aligned {
                text.push(PaintedText {
                    col: run.col,
                    #[cfg(debug_assertions)]
                    columns: run.columns,
                    shaped,
                });
            } else {
                for (offset, ch) in run.text.chars().enumerate() {
                    let s = ch.to_string();
                    let style = self.text_run(&s, run.fg, run.flags);
                    let shaped = window.text_system().shape_line(
                        s.into(),
                        px(self.font_size),
                        &[style],
                        None,
                    );
                    text.push(PaintedText {
                        col: run.col + offset,
                        #[cfg(debug_assertions)]
                        columns: 1,
                        shaped,
                    });
                }
            }
        }
        PaintedRow {
            source,
            text,
            glyphs: plan.glyphs,
            decorations: plan.decorations,
            backgrounds: plan.backgrounds,
        }
    }

    fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Frame {
        let started = Instant::now();
        self.bounds = bounds;
        if self
            .session
            .as_ref()
            .is_some_and(|s| s.revision.load(Ordering::Acquire) != self.snapshot.revision)
            || self.snapshot.rows.is_empty()
        {
            self.request_snapshot(cx);
        }
        let line_height = self
            .line_height_scale
            .measure(window, &self.font_family, self.font_size);
        let font_changed = self.measured_font_size != self.font_size.to_bits()
            || self.measured_font_family != self.font_family
            || self.line_height != line_height;
        if font_changed {
            let run = self.text_run("M", self.palette.text, Flags::empty());
            let probe =
                window
                    .text_system()
                    .shape_line("M".into(), px(self.font_size), &[run], None);
            self.cell_width = f32::from(probe.width).max(1.);
            self.line_height = line_height;
            self.measured_font_size = self.font_size.to_bits();
            self.measured_font_family = self.font_family.clone();
        }
        let style_changed = font_changed
            || self.painted_ligatures != self.ligatures
            || self.painted_palette != self.palette
            || (0..COUNT).any(|i| self.painted_colors[i] != self.snapshot.colors[i]);
        let mut rebuilt = 0;
        self.painted_ligatures = self.ligatures;
        let rows = self
            .snapshot
            .rows
            .iter()
            .enumerate()
            .map(|(row, source)| {
                if !style_changed
                    && let Some(painted) = self.painted_rows.get(row)
                    && Arc::ptr_eq(source, &painted.source)
                {
                    return painted.clone();
                }
                rebuilt += 1;
                Arc::new(self.shape_row(source.clone(), window))
            })
            .collect::<Vec<_>>();
        // Immutable row entities are reactive cache boundaries. A changed row
        // gets a new identity immediately, including during this prepaint pass;
        // unchanged rows keep their identity and reuse GPUI's scene primitives.
        self.row_views = rows
            .iter()
            .enumerate()
            .map(|(row, content)| {
                if content.text.is_empty()
                    && content.glyphs.is_empty()
                    && content.decorations.is_empty()
                {
                    return None;
                }
                if let Some(old) = self.painted_rows.get(row)
                    && Arc::ptr_eq(old, content)
                    && let Some(view) = self.row_views.get(row)
                {
                    return view.clone();
                }
                Some(cx.new(|_| RowTextView {
                    row: content.clone(),
                    cell_width: self.cell_width,
                    line_height: self.line_height,
                    font_size: self.font_size,
                    paints: self.row_paints.clone(),
                }))
            })
            .collect();
        self.painted_rows = rows.clone();
        self.painted_palette = self.palette;
        self.painted_colors = self.snapshot.colors;
        if let Some(cursor) = self.snapshot.cursor {
            self.cursor = (cursor.row, cursor.col);
        }
        let mut selection = selection_spans(
            self.snapshot.selection,
            rows.len(),
            self.snapshot.cols,
            self.snapshot.display_offset,
            self.palette.selection,
        );
        expand_wide_selection(&mut selection, &self.snapshot.rows);
        tracing::trace!(target: "tshell::render", micros = started.elapsed().as_micros() as u64,
            rebuilt_rows = rebuilt, copied_rows = self.snapshot.copied_rows, "prepare");
        let mut text_elements = Vec::with_capacity(rows.len());
        for (row, view) in self.row_views.iter().enumerate() {
            let Some(view) = view else { continue };
            let origin = bounds.origin + point(px(0.), px(row as f32 * self.line_height));
            if origin.y >= bounds.bottom() {
                break;
            }
            let mut element = view
                .clone()
                .cached(
                    StyleRefinement::default()
                        .w(bounds.size.width)
                        .h(px(self.line_height)),
                )
                .into_any_element();
            // The hidden-window check compares cached and uncached drawing with
            // the exact same content, font, renderer and build profile.
            #[cfg(debug_assertions)]
            if !self.cache_row_paint {
                element = view.clone().into_any_element();
            }
            element.layout_as_root(
                size(bounds.size.width, px(self.line_height)).into(),
                window,
                cx,
            );
            element.prepaint_at(origin, window, cx);
            text_elements.push(element);
        }
        Frame {
            rows,
            text_elements,
            selection,
            cursor: self.snapshot.cursor,
            background: resolve_color(
                Color::Named(NamedColor::Background),
                self.palette,
                &self.snapshot.colors,
            ),
        }
    }

    fn paint_span(
        &self,
        bounds: Bounds<Pixels>,
        row: usize,
        span: &PaintSpan,
        window: &mut Window,
    ) {
        let origin = bounds.origin
            + point(
                px(span.col as f32 * self.cell_width),
                px(row as f32 * self.line_height),
            );
        window.paint_quad(fill(
            Bounds::new(
                origin,
                size(
                    px(span.columns as f32 * self.cell_width),
                    px(self.line_height),
                ),
            ),
            rgb(span.color),
        ));
    }

    fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        mut frame: Frame,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        if !self.background_inherited || frame.background != self.palette.terminal {
            window.paint_quad(fill(bounds, rgb(frame.background)));
        }
        for (row, content) in frame.rows.iter().enumerate() {
            for span in &content.backgrounds {
                self.paint_span(bounds, row, span, window);
            }
        }
        for span in &frame.selection {
            self.paint_span(bounds, span.row, span, window);
        }
        let paints_before = self.row_paints.get();
        for element in &mut frame.text_elements {
            element.paint(window, cx);
        }
        tracing::trace!(target: "tshell::render", micros = started.elapsed().as_micros() as u64,
            painted_text_rows = self.row_paints.get() - paints_before,
            text_batches = frame.rows.iter().map(|r| r.text.len()).sum::<usize>(),
            selection_spans = frame.selection.len(), "paint");
        if let Some(cursor) = frame.cursor
            && cursor_should_paint(self.focus.is_focused(window), self.cursor_visible)
        {
            let origin = bounds.origin
                + point(
                    px(cursor.col as f32 * self.cell_width),
                    px(cursor.row as f32 * self.line_height),
                );
            let cell = frame
                .rows
                .get(cursor.row)
                .and_then(|row| row.source.get(cursor.col));
            let (origin, cursor_size) = cursor_geometry(
                origin,
                cursor.shape,
                cell,
                self.cell_width,
                self.line_height,
            );
            window.paint_quad(fill(
                Bounds::new(origin, cursor_size),
                rgba((self.palette.accent << 8) | 0xB0),
            ));
        }
        if !self.preedit.is_empty() {
            let origin = bounds.origin
                + point(
                    px(self.cursor.1 as f32 * self.cell_width),
                    px(self.cursor.0 as f32 * self.line_height),
                );
            let run = TextRun {
                len: self.preedit.len(),
                font: self.terminal_font(),
                color: rgb(self.palette.text).into(),
                background_color: Some(rgb(self.palette.selected).into()),
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(rgb(self.palette.accent).into()),
                    wavy: false,
                }),
                strikethrough: None,
            };
            let shaped = window.text_system().shape_line(
                self.preedit.clone().into(),
                px(self.font_size),
                &[run],
                None,
            );
            window.paint_quad(fill(
                Bounds::new(origin, size(shaped.width, px(self.line_height))),
                rgb(self.palette.selected),
            ));
            let _ = shaped.paint(
                origin,
                px(self.line_height),
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.focus_changed(self.focus.is_focused(window), cx);
        if self.focus_subscriptions.is_empty() {
            let focus = self.focus.clone();
            self.focus_subscriptions = vec![
                cx.on_focus(&focus, window, |this, _, cx| this.focus_changed(true, cx)),
                cx.on_blur(&focus, window, |this, _, cx| this.focus_changed(false, cx)),
            ];
        }
        let prepare_entity = cx.entity();
        let paint_entity = cx.entity();
        let notice = self.failure.clone();
        div()
            .id("terminal-surface")
            .relative()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .cursor(CursorStyle::Arrow)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if !this.preedit.is_empty() {
                    return;
                }
                if event.keystroke.modifiers.shift
                    && matches!(event.keystroke.key.as_str(), "pageup" | "pagedown")
                {
                    this.flush_selection(cx);
                    this.queue_render(
                        RenderCommand::Scroll(if event.keystroke.key == "pageup" {
                            Scroll::PageUp
                        } else {
                            Scroll::PageDown
                        }),
                        cx,
                    );
                    cx.stop_propagation();
                    window.prevent_default();
                    return;
                }
                let mode = this.session_mode();
                if let Some(bytes) = encode_key(&event.keystroke, mode) {
                    this.send(bytes, cx);
                    cx.stop_propagation();
                    window.prevent_default();
                }
            }))
            .on_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                let reported = this.mouse_down(event, window, cx);
                if reported && event.button != MouseButton::Left {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::handle_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::handle_mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::handle_mouse_up))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                this.scroll_pixels += f32::from(event.delta.pixel_delta(px(this.line_height)).y);
                let lines = (this.scroll_pixels / this.line_height).trunc() as i32;
                if lines == 0 {
                    return;
                }
                this.scroll_pixels -= lines as f32 * this.line_height;
                this.flush_selection(cx);
                let mode = this.session_mode();
                if mouse_reporting(mode) && !event.modifiers.shift {
                    let cell = this.viewport_cell(event.position);
                    let report = if lines > 0 {
                        MouseReport::WheelUp
                    } else {
                        MouseReport::WheelDown
                    };
                    let mut bytes = Vec::new();
                    for _ in 0..lines.unsigned_abs().min(15) {
                        if let Some(report) =
                            encode_mouse(report, cell.0, cell.1, &event.modifiers, mode)
                        {
                            bytes.extend(report);
                        }
                    }
                    if !bytes.is_empty() {
                        this.last_mouse_cell = Some(cell);
                        this.queue_render(RenderCommand::Input(bytes), cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                } else if mode.contains(TermMode::ALT_SCREEN) && !event.modifiers.shift {
                    let bytes = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
                    this.queue_render(
                        RenderCommand::Input(bytes.repeat(lines.unsigned_abs().min(15) as usize)),
                        cx,
                    );
                } else {
                    this.queue_render(RenderCommand::Scroll(Scroll::Delta(lines)), cx);
                }
            }))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        prepare_entity.update(cx, |this, cx| this.prepare(bounds, window, cx))
                    },
                    move |bounds, frame, window, cx| {
                        paint_entity.update(cx, |this, cx| this.paint(bounds, frame, window, cx));
                        let pointer_view = paint_entity.downgrade();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase.bubble() {
                                let _ = pointer_view
                                    .update(cx, |this, cx| this.move_selection(event, window, cx));
                            }
                        });
                        let release_view = paint_entity.downgrade();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase.bubble() && event.button == MouseButton::Left {
                                let _ = release_view.update(cx, |this, cx| {
                                    if this.selecting {
                                        this.finish_selection(cx);
                                    }
                                });
                            }
                        });
                        let focus = paint_entity.read(cx).focus.clone();
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, paint_entity),
                            cx,
                        );
                    },
                )
                .size_full(),
            )
            .when_some(notice, |view, notice| {
                view.child(
                    Alert::error("terminal-error", notice)
                        .banner()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .w_full()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .bg(rgb(self.palette.panel))
                        .text_color(rgb(self.palette.error))
                        .border_color(rgb(self.palette.error)),
                )
            })
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let utf16: Vec<_> = self.preedit.encode_utf16().collect();
        let range = range.start.min(utf16.len())..range.end.min(utf16.len());
        *actual = Some(range.clone());
        Some(String::from_utf16_lossy(&utf16[range]))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.preedit_selection.clone(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.preedit.is_empty()).then(|| 0..self.preedit.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.preedit.is_empty() {
            self.preedit.clear();
            self.preedit_selection = 0..0;
            cx.notify();
        }
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preedit.clear();
        self.preedit_selection = 0..0;
        self.send(text.as_bytes().to_vec(), cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preedit = text.into();
        let count = text.encode_utf16().count();
        self.preedit_selection = selected.unwrap_or(count..count);
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(
            self.bounds.origin
                + point(
                    px(self.cursor.1 as f32 * self.cell_width),
                    px(self.cursor.0 as f32 * self.line_height),
                ),
            size(px(self.cell_width), px(self.line_height)),
        ))
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{cursor_geometry, cursor_should_paint};
    use alacritty_terminal::vte::ansi::CursorShape;
    use gpui_kit::{point, px};

    #[test]
    fn focused_cursor_uses_a_standard_block_and_blur_hides_it() {
        let origin = point(px(3.), px(4.));
        let (block_origin, block_size) =
            cursor_geometry(origin, CursorShape::Block, None, 10., 20.);
        assert_eq!(block_origin, origin);
        assert_eq!(block_size, gpui_kit::size(px(10.), px(20.)));
        assert!(cursor_should_paint(true, true));
        assert!(!cursor_should_paint(true, false));
        assert!(!cursor_should_paint(false, true));
    }

    #[test]
    fn input_cursor_covers_chinese_after_horizontal_and_vertical_moves() {
        use crate::terminal::Session;
        use std::{collections::VecDeque, sync::Arc};

        // xterm.js 6.0 WebglRenderer uses cell.getWidth() for the cursor;
        // RectangleRenderer keeps the bar thin even on a two-column cell.
        let session = Session::remote("%wide-cursor".into(), 3, 20, Arc::new(|_| Ok(())));
        session.remote_output("A中文B\r\nX中文Y\x1b[1;2H".as_bytes());
        for (bytes, row, col, width) in [
            ("", 0, 1, 20.),
            ("\x1b[2C", 0, 3, 20.),
            ("\x1b[B", 1, 3, 20.),
            ("\x1b[2D", 1, 1, 20.),
            ("\x1b[A", 0, 1, 20.),
            ("\x1b[D", 0, 0, 10.),
            ("\x1b[6G", 0, 5, 10.),
            ("\x1b[C", 0, 6, 10.),
        ] {
            session.remote_output(bytes.as_bytes());
            let snapshot = session.render_snapshot(VecDeque::new()).0;
            let cursor = snapshot.cursor.unwrap();
            assert_eq!((cursor.row, cursor.col), (row, col));
            let cell = snapshot.rows[row].get(col);
            for (shape, expected) in [
                (CursorShape::Block, width),
                (CursorShape::Underline, width),
                (CursorShape::Beam, 1.),
            ] {
                let (_, size) = cursor_geometry(point(px(0.), px(0.)), shape, cell, 10., 20.);
                assert_eq!(size.width, px(expected), "{bytes:?} {shape:?}");
            }
        }
    }
}

#[cfg(debug_assertions)]
mod render_check;
#[cfg(debug_assertions)]
pub(crate) use render_check::run_render_check;
