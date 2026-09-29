//! Immutable, shared rows for the renderer. Only the background snapshot job reads Term.
use alacritty_terminal::{
    event::EventListener,
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionRange, SelectionType},
    term::{Term, TermDamage, cell::Cell, color::Colors},
    vte::ansi::CursorShape,
};
use std::{collections::VecDeque, sync::Arc, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Anchor {
    pub point: Point,
    pub side: Side,
}

#[derive(Clone)]
pub(crate) enum RenderCommand {
    Begin(Anchor, SelectionType),
    Extend(Anchor),
    Scroll(Scroll),
    DragScroll {
        lines: i32,
        column: usize,
        side: Side,
        block: bool,
    },
    Copy,
    Input(Vec<u8>),
    MouseMove(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RenderCursor {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    pub blinking: bool,
}

/// Bound input work while a snapshot is in flight. Only adjacent, replaceable moves
/// are coalesced; begin, copy, input and scroll retain their ordering.
#[derive(Default)]
pub(crate) struct RenderMailbox(VecDeque<RenderCommand>, usize);

impl RenderMailbox {
    pub fn push(&mut self, command: RenderCommand) -> bool {
        let bytes = match &command {
            RenderCommand::Input(data) | RenderCommand::MouseMove(data) => data.len(),
            _ => 0,
        };
        if bytes > 1024 * 1024 {
            return false;
        }
        if let RenderCommand::MouseMove(data) = &command
            && let Some(RenderCommand::MouseMove(previous)) = self.0.back_mut()
        {
            let pending_bytes = self.1 - previous.len() + data.len();
            if pending_bytes > 2 * 1024 * 1024 {
                return false;
            }
            self.1 = pending_bytes;
            *previous = data.clone();
            return true;
        }
        if self.1 + bytes > 2 * 1024 * 1024 {
            return false;
        }
        if let RenderCommand::Extend(_) = command
            && let Some(last @ RenderCommand::Extend(_)) = self.0.back_mut()
        {
            *last = command;
            return true;
        }
        if self.0.len() == 256 {
            return false;
        }
        self.0.push_back(command);
        self.1 += bytes;
        true
    }

    pub fn take(&mut self) -> VecDeque<RenderCommand> {
        self.1 = 0;
        std::mem::take(&mut self.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Default)]
pub(crate) struct RenderSnapshot {
    pub revision: u64,
    pub rows: Vec<Arc<[Cell]>>,
    pub cols: usize,
    pub display_offset: usize,
    pub selection: Option<SelectionRange>,
    pub cursor: Option<RenderCursor>,
    pub colors: Colors,
    pub copied_rows: usize,
}

impl RenderSnapshot {
    /// Revision changes can contain mode updates or identical output. Only
    /// visible state invalidates the view; input and copy alone need no frame.
    pub fn same_visual(&self, other: &Self) -> bool {
        self.cols == other.cols
            && self.display_offset == other.display_offset
            && self.selection == other.selection
            && self.cursor == other.cursor
            && self.rows.len() == other.rows.len()
            && self
                .rows
                .iter()
                .zip(&other.rows)
                .all(|(a, b)| Arc::ptr_eq(a, b))
            && (0..alacritty_terminal::term::color::COUNT)
                .all(|i| self.colors[i] == other.colors[i])
    }

    pub fn capture<T: EventListener>(term: &mut Term<T>, revision: u64, previous: &Self) -> Self {
        let started = Instant::now();
        let rows = term.screen_lines();
        let cols = term.columns();
        let offset = term.grid().display_offset();
        let full = previous.rows.len() != rows
            || previous.cols != cols
            || previous.display_offset != offset;
        let mut dirty = vec![full; rows];
        // Damage is consumed centrally by Session, never by individual views.
        match term.damage() {
            TermDamage::Full => dirty.fill(true),
            TermDamage::Partial(lines) => {
                for line in lines {
                    if let Some(dirty) = dirty.get_mut(line.line) {
                        *dirty = true;
                    }
                }
            }
        }
        let mut copied_rows = 0;
        let cursor_blinking = term.cursor_style().blinking;
        let content = term.renderable_content();
        let selection = content.selection;
        let cursor =
            (content.cursor.shape != CursorShape::Hidden && offset == 0).then_some(RenderCursor {
                row: content.cursor.point.line.0.max(0) as usize,
                col: content.cursor.point.column.0,
                shape: content.cursor.shape,
                blinking: cursor_blinking,
            });
        let colors = *content.colors;
        let row_data = (0..rows)
            .map(|row| {
                let line = Line(row as i32 - offset as i32);
                let old = previous.rows.get(row).filter(|old| old.len() == cols);
                if !full
                    && let Some(old) = old
                    && (!dirty[row]
                        || (0..cols).all(|col| old[col] == term.grid()[line][Column(col)]))
                {
                    return old.clone();
                }
                copied_rows += 1;
                (0..cols)
                    .map(|col| term.grid()[line][Column(col)].clone())
                    .collect::<Arc<[Cell]>>()
            })
            .collect();
        term.reset_damage();
        tracing::trace!(target: "tshell::render", micros = started.elapsed().as_micros() as u64,
            rows, cols, copied_rows, "snapshot");
        Self {
            revision,
            rows: row_data,
            cols,
            display_offset: offset,
            selection,
            cursor,
            colors,
            copied_rows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Session;
    use alacritty_terminal::term::TermMode;

    fn session(rows: usize, cols: usize) -> Arc<Session> {
        Session::remote("%render-test".into(), rows, cols, Arc::new(|_| Ok(())))
    }
    fn anchor(row: i32, col: usize, side: Side) -> Anchor {
        Anchor {
            point: Point::new(Line(row), Column(col)),
            side,
        }
    }
    fn assert_matches_grid(session: &Session, snapshot: &RenderSnapshot) {
        let term = session.term.lock();
        assert_eq!(snapshot.cols, term.columns());
        assert_eq!(snapshot.rows.len(), term.screen_lines());
        for (row, cells) in snapshot.rows.iter().enumerate() {
            for (col, cell) in cells.iter().enumerate() {
                assert_eq!(
                    cell,
                    &term.grid()[Line(row as i32 - snapshot.display_offset as i32)][Column(col)]
                );
            }
        }
    }

    #[test]
    fn drag_scroll_extends_selection_using_the_updated_viewport() {
        let s = session(5, 20);
        for row in 0..30 {
            s.remote_output(format!("line {row}\r\n").as_bytes());
        }
        let (before, _) = s.render_snapshot(VecDeque::new());
        let (after, _) = s.render_snapshot(VecDeque::from([
            RenderCommand::Begin(anchor(2, 3, Side::Left), SelectionType::Simple),
            RenderCommand::DragScroll {
                lines: 3,
                column: 8,
                side: Side::Right,
                block: false,
            },
        ]));
        assert_eq!(after.display_offset, before.display_offset + 3);
        let selection = after.selection.unwrap();
        assert_eq!(selection.start, Point::new(Line(-3), Column(0)));
        let (bottom, _) = s.render_snapshot(VecDeque::from([RenderCommand::DragScroll {
            lines: -3,
            column: 8,
            side: Side::Right,
            block: false,
        }]));
        assert_eq!(bottom.display_offset, 0);
        assert_eq!(
            bottom.selection.unwrap().end,
            Point::new(Line(4), Column(19))
        );
    }

    #[test]
    fn selection_only_reuses_every_row_and_copy_preserves_order() {
        let s = session(24, 80);
        s.remote_output(b"abcdefgh");
        let (before, _) = s.render_snapshot(VecDeque::new());
        let (after, copies) = s.render_snapshot(VecDeque::from([
            RenderCommand::Begin(anchor(0, 1, Side::Left), SelectionType::Simple),
            RenderCommand::Extend(anchor(0, 4, Side::Right)),
            RenderCommand::Copy,
            RenderCommand::Input(Vec::new()),
        ]));
        assert_eq!(copies, ["bcde"]);
        assert!(after.selection.is_none());
        assert_eq!(after.copied_rows, 0);
        assert!(
            before
                .rows
                .iter()
                .zip(&after.rows)
                .all(|(a, b)| Arc::ptr_eq(a, b))
        );
    }

    #[test]
    fn mouse_motion_preserves_scrollback_and_selection() {
        let s = session(5, 20);
        for row in 0..30 {
            s.remote_output(format!("line {row}\r\n").as_bytes());
        }
        let (before, _) = s.render_snapshot(VecDeque::from([
            RenderCommand::Scroll(Scroll::Delta(3)),
            RenderCommand::Begin(anchor(1, 1, Side::Left), SelectionType::Simple),
            RenderCommand::Extend(anchor(1, 4, Side::Right)),
        ]));
        let (after, _) = s.render_snapshot(VecDeque::from([RenderCommand::MouseMove(
            b"mouse".to_vec(),
        )]));
        assert_eq!(after.display_offset, before.display_offset);
        assert_eq!(after.selection, before.selection);
    }

    #[test]
    fn sparse_output_replaces_one_row_without_mutating_previous_snapshot() {
        let s = session(40, 120);
        let (before, _) = s.render_snapshot(VecDeque::new());
        s.remote_output(b"\x1b[15;21HX");
        let (after, _) = s.render_snapshot(VecDeque::new());
        assert_eq!(before.rows[14][20].c, ' ');
        assert_eq!(after.rows[14][20].c, 'X');
        assert_eq!(after.copied_rows, 1);
        assert!(
            before
                .rows
                .iter()
                .zip(&after.rows)
                .enumerate()
                .all(|(i, (a, b))| i == 14 || Arc::ptr_eq(a, b))
        );
        assert_matches_grid(&s, &after);
    }

    #[test]
    fn snapshots_follow_scrollback_resize_and_alternate_screen() {
        let s = session(6, 20);
        for i in 0..20 {
            s.remote_output(format!("line {i}\r\n").as_bytes());
        }
        s.render_snapshot(VecDeque::new());
        let (scrolled, _) =
            s.render_snapshot(VecDeque::from([RenderCommand::Scroll(Scroll::Delta(4))]));
        assert_eq!(scrolled.display_offset, 4);
        assert_matches_grid(&s, &scrolled);
        s.remote_output(b"more output\r\n");
        let (updated, _) = s.render_snapshot(VecDeque::new());
        assert_matches_grid(&s, &updated);
        s.remote_resize(9, 32);
        let (resized, _) = s.render_snapshot(VecDeque::new());
        assert_matches_grid(&s, &resized);
        s.remote_output(b"\x1b[?1049hALT\x1b[3;3Hhere");
        let (alt, _) = s.render_snapshot(VecDeque::new());
        assert!(s.mode().contains(TermMode::ALT_SCREEN));
        assert_matches_grid(&s, &alt);
        s.remote_output(b"\x1b[?1049l");
        let (restored, _) = s.render_snapshot(VecDeque::new());
        assert!(!s.mode().contains(TermMode::ALT_SCREEN));
        assert_matches_grid(&s, &restored);
    }

    #[test]
    fn mailbox_coalesces_moves_but_keeps_copy_barriers_and_is_bounded() {
        let mut mailbox = RenderMailbox::default();
        for i in 0..10_000 {
            assert!(mailbox.push(RenderCommand::Extend(anchor(0, i, Side::Right))));
        }
        assert_eq!(mailbox.0.len(), 1);
        assert!(mailbox.push(RenderCommand::Copy));
        assert!(mailbox.push(RenderCommand::Extend(anchor(1, 2, Side::Left))));
        let commands = mailbox.take();
        assert!(matches!(commands[0], RenderCommand::Extend(a) if a.point.column.0 == 9999));
        assert!(matches!(commands[1], RenderCommand::Copy));
        for _ in 0..256 {
            assert!(mailbox.push(RenderCommand::Copy));
        }
        assert!(!mailbox.push(RenderCommand::Copy));
        mailbox.take();
        assert!(!mailbox.push(RenderCommand::Input(vec![0; 1024 * 1024 + 1])));
        assert!(mailbox.push(RenderCommand::Input(vec![0; 1024 * 1024])));
        assert!(mailbox.push(RenderCommand::Input(vec![0; 1024 * 1024])));
        assert!(!mailbox.push(RenderCommand::Input(vec![0])));
        mailbox.take();
        assert!(mailbox.push(RenderCommand::Input(vec![1])));
    }

    #[test]
    fn mailbox_coalesces_mouse_motion_without_crossing_input_barriers() {
        let mut mailbox = RenderMailbox::default();
        assert!(mailbox.push(RenderCommand::Input(b"press".to_vec())));
        for i in 0..10_000_u32 {
            assert!(mailbox.push(RenderCommand::MouseMove(i.to_be_bytes().to_vec())));
        }
        assert!(mailbox.push(RenderCommand::Input(b"release".to_vec())));
        let commands = mailbox.take();
        assert_eq!(commands.len(), 3);
        assert!(matches!(&commands[0], RenderCommand::Input(data) if data == b"press"));
        assert!(
            matches!(&commands[1], RenderCommand::MouseMove(data) if data == &9_999_u32.to_be_bytes())
        );
        assert!(matches!(&commands[2], RenderCommand::Input(data) if data == b"release"));
    }

    #[test]
    fn input_echo_can_arrive_before_snapshot_capture() {
        let target = Arc::new(parking_lot::Mutex::new(std::sync::Weak::<Session>::new()));
        let echo = target.clone();
        let session = Session::remote(
            "%immediate-echo".into(),
            4,
            20,
            Arc::new(move |bytes| {
                let session = echo.lock().upgrade().unwrap();
                // A transport may publish output immediately. No grid lock may
                // be held while invoking it, and this capture must see the echo.
                assert!(session.term.try_lock().is_some());
                session.remote_output(&bytes);
                Ok(())
            }),
        );
        *target.lock() = Arc::downgrade(&session);
        let (snapshot, _) = session.render_snapshot(VecDeque::from([
            RenderCommand::Input(b"first".to_vec()),
            RenderCommand::Input(b"second".to_vec()),
        ]));
        let text: String = snapshot.rows[0].iter().map(|cell| cell.c).collect();
        assert!(text.starts_with("firstsecond"));
        assert_matches_grid(&session, &snapshot);
    }

    #[test]
    fn input_follows_copy_and_uses_latest_modes_without_rendering() {
        let written = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let output = written.clone();
        let s = Session::remote(
            "%order-test".into(),
            4,
            20,
            Arc::new(move |bytes| {
                output.lock().push(bytes);
                Ok(())
            }),
        );
        s.remote_output(b"hello\x1b[?2004h\x1b[?1h");
        assert!(
            s.mode()
                .contains(TermMode::BRACKETED_PASTE | TermMode::APP_CURSOR)
        );
        let (snap, copies) = s.render_snapshot(VecDeque::from([
            RenderCommand::Begin(anchor(0, 0, Side::Left), SelectionType::Simple),
            RenderCommand::Extend(anchor(0, 4, Side::Right)),
            RenderCommand::Copy,
            RenderCommand::Input(b"one".to_vec()),
            RenderCommand::Input(b"two".to_vec()),
        ]));
        assert_eq!(copies, ["hello"]);
        assert!(snap.selection.is_none());
        assert_eq!(*written.lock(), vec![b"one".to_vec(), b"two".to_vec()]);
        s.remote_output(b"\x1b[?2004l\x1b[?1l");
        assert!(
            !s.mode()
                .intersects(TermMode::BRACKETED_PASTE | TermMode::APP_CURSOR)
        );
    }

    #[test]
    fn visual_equality_ignores_revision_but_tracks_cursor_selection_and_colors() {
        let s = session(4, 20);
        s.remote_output(b"hello");
        let (before, _) = s.render_snapshot(VecDeque::new());
        s.remote_output(b"\x1b[?2004h");
        let (mode_only, _) = s.render_snapshot(VecDeque::new());
        assert_ne!(before.revision, mode_only.revision);
        assert!(before.same_visual(&mode_only));
        let (input, _) = s.render_snapshot(VecDeque::from([RenderCommand::Input(b"j".to_vec())]));
        assert!(input.same_visual(&mode_only));
        s.remote_output(b"\x1b[1D");
        let (cursor, _) = s.render_snapshot(VecDeque::new());
        assert!(!cursor.same_visual(&input));
        assert_eq!(cursor.copied_rows, 0);
        let (selection, _) = s.render_snapshot(VecDeque::from([
            RenderCommand::Begin(anchor(0, 0, Side::Left), SelectionType::Simple),
            RenderCommand::Extend(anchor(0, 2, Side::Right)),
        ]));
        assert!(!selection.same_visual(&cursor));
        s.remote_output(b"\x1b]10;#ff0000\x07");
        let (color, _) = s.render_snapshot(VecDeque::new());
        assert!(!color.same_visual(&selection));
    }

    #[test]
    fn cursor_defaults_and_decscusr_styles_are_preserved() {
        let s = session(4, 20);
        let (default, _) = s.render_snapshot(VecDeque::new());
        assert_eq!(
            default.cursor.map(|cursor| (cursor.shape, cursor.blinking)),
            Some((CursorShape::Block, true))
        );

        for (sequence, expected) in [
            (b"\x1b[2 q".as_slice(), (CursorShape::Block, false)),
            (b"\x1b[3 q".as_slice(), (CursorShape::Underline, true)),
            (b"\x1b[6 q".as_slice(), (CursorShape::Beam, false)),
            (b"\x1b[0 q".as_slice(), (CursorShape::Block, true)),
        ] {
            s.remote_output(sequence);
            let (snapshot, _) = s.render_snapshot(VecDeque::new());
            assert_eq!(
                snapshot
                    .cursor
                    .map(|cursor| (cursor.shape, cursor.blinking)),
                Some(expected),
                "{sequence:?}"
            );
        }
    }

    #[test]
    fn concurrent_output_and_snapshot_readers_do_not_lose_damage() {
        let s = session(24, 80);
        let writer = s.clone();
        let thread = std::thread::spawn(move || {
            for i in 0..500 {
                writer.remote_output(format!("\x1b[{};1H{:04}", i % 24 + 1, i).as_bytes());
            }
        });
        for _ in 0..100 {
            let (snap, _) = s.render_snapshot(VecDeque::new());
            assert_eq!(snap.rows.len(), 24);
        }
        thread.join().unwrap();
        let (final_frame, _) = s.render_snapshot(VecDeque::new());
        assert_matches_grid(&s, &final_frame);
    }

    #[test]
    #[ignore = "opt-in CPU benchmark; does not measure GPU presentation"]
    fn render_snapshot_benchmark() {
        let s = session(60, 200);
        for i in 0..60 {
            s.remote_output(format!("\x1b[{};1H{}", i + 1, "x".repeat(190)).as_bytes());
        }
        s.render_snapshot(VecDeque::new());
        let mut samples = Vec::new();
        let mut copies = 0;
        for i in 0..2_000 {
            let start = Instant::now();
            let (snapshot, _) = s.render_snapshot(VecDeque::from([
                RenderCommand::Begin(anchor(0, 0, Side::Left), SelectionType::Simple),
                RenderCommand::Extend(anchor(i % 60, (i as usize) % 200, Side::Right)),
            ]));
            samples.push(start.elapsed().as_micros());
            copies += snapshot.copied_rows;
        }
        samples.sort_unstable();
        assert_eq!(copies, 0);
        println!(
            "selection snapshots 200x60: n=2000 median={}us p95={}us p99={}us copied_rows={copies}",
            samples[1000], samples[1900], samples[1980]
        );
    }
}

pub(crate) fn apply_commands<T: EventListener>(
    term: &mut Term<T>,
    commands: VecDeque<RenderCommand>,
) -> (Vec<String>, Vec<Vec<u8>>) {
    let mut copies = Vec::new();
    let mut inputs = Vec::new();
    for command in commands {
        match command {
            RenderCommand::Begin(anchor, ty) => {
                term.selection = Some(Selection::new(ty, anchor.point, anchor.side));
            }
            RenderCommand::Extend(anchor) => {
                if let Some(selection) = term.selection.as_mut() {
                    selection.update(anchor.point, anchor.side);
                }
            }
            RenderCommand::Scroll(scroll) => term.scroll_display(scroll),
            RenderCommand::DragScroll {
                lines,
                column,
                side,
                block,
            } => {
                term.scroll_display(Scroll::Delta(lines));
                let top = -(term.grid().display_offset() as i32);
                let row = if lines > 0 {
                    top
                } else {
                    (top + term.screen_lines() as i32).min(term.bottommost_line().0)
                };
                let (column, side) = if block {
                    (column.min(term.columns() - 1), side)
                } else if lines > 0 {
                    (0, Side::Left)
                } else {
                    (term.columns() - 1, Side::Right)
                };
                if let Some(selection) = term.selection.as_mut() {
                    selection.update(Point::new(Line(row), Column(column)), side);
                }
            }
            RenderCommand::Copy => {
                if let Some(text) = term.selection_to_string() {
                    copies.push(text);
                }
            }
            RenderCommand::Input(data) => {
                term.scroll_display(Scroll::Bottom);
                term.selection = None;
                inputs.push(data);
            }
            RenderCommand::MouseMove(data) => inputs.push(data),
        }
    }
    (copies, inputs)
}
