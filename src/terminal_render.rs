use crate::appearance::Palette;
use crate::terminal_snapshot::Anchor;
use alacritty_terminal::{
    index::{Column, Line, Point, Side},
    selection::SelectionRange,
    term::{
        cell::{Cell, Flags},
        color::Colors,
    },
    vte::ansi::{Color, NamedColor},
};

#[derive(Debug)]
pub(crate) struct TextRunPlan {
    pub col: usize,
    pub columns: usize,
    pub text: String,
    pub fg: u32,
    pub flags: Flags,
    // Only printable ASCII without combining marks is shaped across cell boundaries.
    pub ascii: bool,
}

#[derive(Default)]
pub(crate) struct RowPlan {
    pub text: Vec<TextRunPlan>,
    pub glyphs: Vec<crate::terminal_glyphs::GlyphCell>,
    pub decorations: Vec<crate::terminal_decorations::Decoration>,
    pub backgrounds: Vec<PaintSpan>,
}

fn foreground_color(value: Color, bold: bool, palette: Palette, colors: &Colors) -> u32 {
    // xterm drawBoldTextInBrightColors=true promotes displayed palette entries
    // 0..7 only. Defaults, explicit RGB and background colours are not promoted.
    let value = match value {
        Color::Indexed(i @ 0..=7) if bold => Color::Indexed(i + 8),
        Color::Named(i) if bold && (i as usize) < 8 => Color::Indexed(i as u8 + 8),
        value => value,
    };
    resolve_color(value, palette, colors)
}

pub(crate) fn resolve_color(value: Color, palette: Palette, colors: &Colors) -> u32 {
    let index = match value {
        Color::Named(named) => Some(named as usize),
        Color::Indexed(index) => Some(index as usize),
        Color::Spec(_) => None,
    };
    if let Some(rgb) = index.and_then(|i| colors[i]) {
        return ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32;
    }
    match value {
        Color::Spec(rgb) => ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32,
        Color::Named(NamedColor::Background) => palette.terminal,
        Color::Named(NamedColor::Foreground | NamedColor::BrightForeground) => palette.text,
        Color::Named(NamedColor::DimForeground) => dim(
            resolve_color(Color::Named(NamedColor::Foreground), palette, colors),
            resolve_color(Color::Named(NamedColor::Background), palette, colors),
        ),
        Color::Named(named) => {
            let index = named as usize;
            if (259..267).contains(&index) {
                dim(
                    resolve_color(Color::Indexed((index - 259) as u8), palette, colors),
                    resolve_color(Color::Named(NamedColor::Background), palette, colors),
                )
            } else {
                palette.ansi.get(index).copied().unwrap_or(palette.text)
            }
        }
        Color::Indexed(index) => crate::terminal_protocol::indexed_color(index, &palette.ansi),
    }
}

fn dim(foreground: u32, background: u32) -> u32 {
    // Fallback for the engine's internal named dim colours. SGR 2 retains
    // foreground RGB and uses ink alpha at paint time instead of this helper.
    let channel =
        |shift: u32| (((foreground >> shift) & 255) + ((background >> shift) & 255) + 1) / 2;
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

pub(crate) fn build_row(cells: &[Cell], palette: Palette, colors: &Colors) -> RowPlan {
    let mut plan = RowPlan::default();
    let terminal_bg = resolve_color(Color::Named(NamedColor::Background), palette, colors);
    for (col, cell) in cells.iter().enumerate() {
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let columns = if cell.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };
        let columns = columns.min(cells.len() - col);
        let (front, back) = if cell.flags.contains(Flags::INVERSE) {
            (cell.bg, cell.fg)
        } else {
            (cell.fg, cell.bg)
        };
        let bold = cell.flags.contains(Flags::BOLD);
        let faint = cell.flags.contains(Flags::DIM);
        let fg = foreground_color(front, bold, palette, colors);
        let bg = resolve_color(back, palette, colors);
        if bg != terminal_bg {
            push_span(
                &mut plan.backgrounds,
                PaintSpan {
                    row: 0,
                    col,
                    columns,
                    color: bg,
                },
            );
        }
        if cell.flags.contains(Flags::HIDDEN) {
            continue;
        }
        let flags = cell.flags
            & (Flags::BOLD | Flags::DIM | Flags::ITALIC | Flags::ALL_UNDERLINES | Flags::STRIKEOUT);
        let underline_color = cell
            .underline_color()
            .map(|c| foreground_color(c, bold, palette, colors))
            .unwrap_or(fg);
        if let Some(kind) = crate::terminal_decorations::Underline::from_flags(flags) {
            plan.decorations
                .push(crate::terminal_decorations::Decoration {
                    col,
                    columns,
                    kind: Some(kind),
                    color: underline_color,
                    faint: faint && cell.underline_color().is_none(),
                });
        }
        if flags.contains(Flags::STRIKEOUT) {
            plan.decorations
                .push(crate::terminal_decorations::Decoration {
                    col,
                    columns,
                    kind: None,
                    color: fg,
                    faint,
                });
        }
        if columns == 1
            && crate::terminal_glyphs::supported(cell.c)
            && cell.zerowidth().is_none_or(|extra| extra.is_empty())
        {
            plan.glyphs.push(crate::terminal_glyphs::GlyphCell {
                col,
                ch: cell.c,
                color: fg,
                faint,
            });
            continue;
        }
        let ascii = cell.c.is_ascii()
            && !cell.c.is_ascii_control()
            && columns == 1
            && cell.zerowidth().is_none_or(|extra| extra.is_empty());
        if ascii
            && let Some(last) = plan.text.last_mut()
            && last.ascii
            && last.col + last.columns == col
            && last.fg == fg
            && last.flags == flags
        {
            last.text.push(cell.c);
            last.columns += 1;
        } else {
            let mut text = cell.c.to_string();
            if let Some(extra) = cell.zerowidth() {
                text.extend(extra);
            }
            plan.text.push(TextRunPlan {
                col,
                columns,
                text,
                fg,
                flags,
                ascii,
            });
        }
    }
    for run in &mut plan.text {
        if run.ascii
            && !run
                .flags
                .intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
        {
            let trimmed = run.text.trim_end_matches(' ').len();
            run.text.truncate(trimmed);
            run.columns = trimmed;
        }
    }
    plan.text.retain(|run| !run.text.is_empty());
    plan
}

pub(crate) fn hit_test(
    x: f32,
    y: f32,
    cell_width: f32,
    line_height: f32,
    rows: usize,
    cols: usize,
    display_offset: usize,
) -> Anchor {
    let x = x.max(0.) / cell_width.max(1.);
    let col = (x.floor() as usize).min(cols.saturating_sub(1));
    let row = ((y.max(0.) / line_height.max(1.)) as usize).min(rows.saturating_sub(1));
    let side = if x - col as f32 >= 0.5 {
        Side::Right
    } else {
        Side::Left
    };
    Anchor {
        point: Point::new(Line(row as i32 - display_offset as i32), Column(col)),
        side,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PaintSpan {
    pub row: usize,
    pub col: usize,
    pub columns: usize,
    pub color: u32,
}

pub(crate) fn push_span(spans: &mut Vec<PaintSpan>, span: PaintSpan) {
    if let Some(previous) = spans.last_mut()
        && previous.row == span.row
        && previous.color == span.color
        && previous.col + previous.columns == span.col
    {
        previous.columns += span.columns;
        return;
    }
    spans.push(span);
}

pub(crate) fn selection_spans(
    selection: Option<SelectionRange>,
    rows: usize,
    cols: usize,
    display_offset: usize,
    color: u32,
) -> Vec<PaintSpan> {
    let Some(selection) = selection else {
        return Vec::new();
    };
    if rows == 0 || cols == 0 {
        return Vec::new();
    }
    let mut spans = Vec::with_capacity(rows);
    for row in 0..rows {
        let line = Line(row as i32 - display_offset as i32);
        if line < selection.start.line || line > selection.end.line {
            continue;
        }
        let (start, end) = if selection.is_block {
            (selection.start.column.0, selection.end.column.0)
        } else {
            (
                if line == selection.start.line {
                    selection.start.column.0
                } else {
                    0
                },
                if line == selection.end.line {
                    selection.end.column.0
                } else {
                    cols - 1
                },
            )
        };
        if start >= cols {
            continue;
        }
        let end = end.min(cols - 1);
        if start <= end {
            spans.push(PaintSpan {
                row,
                col: start,
                columns: end - start + 1,
                color,
            });
        }
    }
    spans
}

pub(crate) fn expand_wide_selection(spans: &mut [PaintSpan], rows: &[std::sync::Arc<[Cell]>]) {
    for span in spans {
        let cells = &rows[span.row];
        let end = (span.col + span.columns).min(cells.len());
        if span.col > 0 && cells[span.col].flags.contains(Flags::WIDE_CHAR_SPACER) {
            span.col -= 1;
        }
        let end = if end > 0 && cells[end - 1].flags.contains(Flags::WIDE_CHAR) {
            (end + 1).min(cells.len())
        } else {
            end
        };
        span.columns = end - span.col;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Session;
    use alacritty_terminal::index::{Column, Point as GridPoint};
    use gpui_kit::component::ThemeMode;
    use std::{collections::VecDeque, sync::Arc};

    #[test]
    fn sgr_decoration_styles_and_explicit_colour_match_xterm_rules() {
        use crate::terminal_decorations::Underline::*;
        let s = Session::remote("%decorations".into(), 4, 80, Arc::new(|_| Ok(())));
        s.remote_output("\x1b[4:1mA\x1b[4:2mB\x1b[4:3mC\x1b[4:4mD\x1b[4:5mE\x1b[0;4;2m│\x1b[1;58:5:1mF\x1b[0;31;44;1;7mG\x1b[0;4:0mH".as_bytes());
        let (snap, _) = s.render_snapshot(VecDeque::new());
        let p = Palette::new(ThemeMode::Light);
        let plan = build_row(&snap.rows[0], p, &snap.colors);
        assert_eq!(
            plan.decorations.iter().map(|d| d.kind).collect::<Vec<_>>(),
            [Single, Double, Curly, Dotted, Dashed, Single, Single].map(Some)
        );
        assert!(plan.glyphs[0].faint);
        assert!(plan.decorations[5].faint);
        assert!(!plan.decorations[6].faint);
        assert_eq!(plan.decorations[6].color, p.ansi[9]);
        assert_eq!(
            plan.text.iter().find(|r| r.text.contains('G')).unwrap().fg,
            p.ansi[12]
        );
        assert_eq!(
            plan.backgrounds.iter().find(|r| r.col == 7).unwrap().color,
            p.ansi[1]
        );
    }

    #[test]
    fn shell_and_ash_blue_encodings_match_across_themes_and_osc_overrides() {
        let light = Palette::new(ThemeMode::Light);
        let dark = Palette::new(ThemeMode::Dark);
        // Appearance switches preserve every indexed colour, both on screen
        // and in OSC query replies. Default foreground/background still adapt.
        for index in 0..=255 {
            assert_eq!(
                resolve_color(Color::Indexed(index), light, &Colors::default()),
                resolve_color(Color::Indexed(index), dark, &Colors::default()),
            );
            assert_eq!(
                light.terminal_theme().rgb(index as usize),
                dark.terminal_theme().rgb(index as usize)
            );
        }
        assert_ne!(light.text, dark.text);
        assert_ne!(light.terminal, dark.terminal);
        let s = Session::remote("%ash-blue".into(), 4, 80, Arc::new(|_| Ok(())));
        // Ratatui Blue -> Crossterm DarkBlue -> SGR 38;5;4. The shell's
        // SGR 34 must be identical, including bold, DIM and SGR 22 resets.
        s.remote_output(b"\x1b[34mA\x1b[38;5;4mB\x1b[2;34mC\x1b[38;5;4mD\x1b[22;34mE\x1b[38;5;4mF\x1b[1;34mG\x1b[38;5;4mH\x1b[0;94mI\x1b[38;5;12mJ\x1b[0;38;2;36;92;190mK");
        for overridden in [false, true] {
            if overridden {
                s.remote_output(b"\x1b]4;4;#1234ab;12;#5678ef\x07");
            }
            let (snapshot, _) = s.render_snapshot(VecDeque::new());
            for mode in [ThemeMode::Light, ThemeMode::Dark] {
                let palette = Palette::new(mode);
                let plan = build_row(&snapshot.rows[0], palette, &snapshot.colors);
                let run = |ch| plan.text.iter().find(|r| r.text.contains(ch)).unwrap();
                for (shell, ash) in [('A', 'B'), ('C', 'D'), ('E', 'F'), ('G', 'H'), ('I', 'J')] {
                    assert_eq!(run(shell).fg, run(ash).fg);
                    assert_eq!(run(shell).flags, run(ash).flags);
                }
                assert_eq!(
                    run('A').fg,
                    if overridden {
                        0x1234ab
                    } else {
                        palette.ansi[4]
                    }
                );
                assert_eq!(
                    run('G').fg,
                    if overridden {
                        0x5678ef
                    } else {
                        palette.ansi[12]
                    }
                );
                assert_eq!(run('K').fg, 0x245cbe); // RGB stays literal on theme changes.
                assert!(run('C').flags.contains(Flags::DIM));
                assert!(!run('E').flags.contains(Flags::DIM));
            }
        }
    }

    #[test]
    fn ash_faint_styles_reduce_contrast_in_both_themes() {
        let s = Session::remote("%ash-styles".into(), 4, 80, Arc::new(|_| Ok(())));
        // ash: plain body, DIM subtitle/tool output, Cyan+DIM code label,
        // BOLD+DIM user marker. Include reverse/truecolour and custom borders.
        s.remote_output(b"A\x1b[2mB\x1b[22mC\x1b[36;2mD\x1b[22mE\x1b[0;2;7mF\x1b[0;38;2;32;64;96;48;2;240;224;208;2mG\x1b[0;1;2mH\x1b[0;2m");
        s.remote_output("│\x1b[0;4;2;58;2;32;64;96mU".as_bytes());
        let (snapshot, _) = s.render_snapshot(VecDeque::new());
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            // Keep the rendering fixture independent of workspace UI text colours.
            let p = Palette {
                text: if mode == ThemeMode::Light {
                    0x273244
                } else {
                    0xdbe4f0
                },
                ..Palette::new(mode)
            };
            let plan = build_row(&snapshot.rows[0], p, &snapshot.colors);
            let run = |ch| plan.text.iter().find(|r| r.text.contains(ch)).unwrap();
            assert_eq!(run('A').fg, p.text);
            assert_eq!(run('C').fg, p.text);
            assert_eq!(run('B').fg, p.text);
            assert!(run('B').flags.contains(Flags::DIM));
            assert_eq!(crate::terminal_decorations::ink(run('B').fg, true).a, 0.5);
            assert_eq!(run('D').fg, p.ansi[6]);
            assert_eq!(run('E').fg, p.ansi[6]);
            assert_eq!(run('F').fg, p.terminal);
            assert_eq!(
                plan.backgrounds.iter().find(|r| r.col == 5).unwrap().color,
                p.text
            );
            assert_eq!(run('G').fg, 0x204060);
            assert_eq!(run('H').fg, run('B').fg);
            assert!(run('H').flags.contains(Flags::BOLD));
            assert_eq!(plan.glyphs[0].color, run('B').fg);
            assert!(plan.glyphs[0].faint);
            assert_eq!(plan.decorations[0].color, 0x204060);
            assert!(!plan.decorations[0].faint); // Explicit SGR 58 colour stays opaque in xterm.
            if mode == ThemeMode::Light {
                assert_eq!(dim(run('B').fg, p.terminal), 0x9399a2);
                assert!(dim(run('B').fg, p.terminal) > run('A').fg);
            } else {
                assert!(dim(run('B').fg, p.terminal) < run('A').fg);
            }
        }
        // Live OSC defaults also affect faint text; theme notification is not
        // required by ash because it uses default/ANSI colours, not a theme API.
        s.remote_output(b"\x1b]10;#204060\x07\x1b]11;#f0e0d0\x07");
        let (snapshot, _) = s.render_snapshot(VecDeque::new());
        let plan = build_row(
            &snapshot.rows[0],
            Palette::new(ThemeMode::Light),
            &snapshot.colors,
        );
        assert_eq!(
            plan.text.iter().find(|r| r.text.contains('B')).unwrap().fg,
            0x204060
        );
    }

    #[test]
    fn tui_graphics_keep_original_text_colors_and_hidden_semantics() {
        let s = Session::remote("%graphics".into(), 4, 40, Arc::new(|_| Ok(())));
        // DEC line drawing is translated by the terminal parser to Unicode.
        s.remote_output("\x1b[31m│─┌╭║█\x1b[8m│\x1b[0m|¦\x1b(0xq\x1b(B".as_bytes());
        let (snap, _) = s.render_snapshot(VecDeque::new());
        let p = Palette::new(ThemeMode::Dark);
        let plan = build_row(&snap.rows[0], p, &snap.colors);
        assert_eq!(
            plan.glyphs.iter().map(|g| g.ch).collect::<String>(),
            "│─┌╭║█│─"
        );
        assert!(plan.glyphs[..6].iter().all(|g| g.color == p.ansi[1]));
        assert!(plan.text.iter().any(|r| r.text.contains('|')));
        assert_eq!(snap.rows[0][0].c, '│');
        assert_eq!(snap.rows[0][9].c, '│');
    }

    #[test]
    fn mixed_width_and_combining_text_keeps_terminal_cell_boundaries() {
        let s = Session::remote("%text-test".into(), 4, 40, Arc::new(|_| Ok(())));
        s.remote_output("ab中e\u{301}🙂Z".as_bytes());
        let (snap, _) = s.render_snapshot(VecDeque::new());
        let plan = build_row(&snap.rows[0], Palette::new(ThemeMode::Dark), &snap.colors);
        let runs: Vec<_> = plan
            .text
            .iter()
            .map(|r| (r.col, r.columns, r.text.as_str(), r.ascii))
            .collect();
        assert_eq!(
            runs,
            vec![
                (0, 2, "ab", true),
                (2, 2, "中", false),
                (4, 1, "e\u{301}", false),
                (5, 2, "🙂", false),
                (7, 1, "Z", true)
            ]
        );
        let mut spans = selection_spans(Some(range((0, 3), (0, 3), false)), 4, 40, 0, 7);
        expand_wide_selection(&mut spans, &snap.rows);
        assert_eq!((spans[0].col, spans[0].columns), (2, 2));
    }

    #[test]
    fn pause_symbols_keep_their_columns_even_when_the_font_overhangs() {
        let s = Session::remote("%pause-test".into(), 2, 20, Arc::new(|_| Ok(())));
        s.remote_output("⏸ ⏸\u{fe0e} ⏸\u{fe0f} 🙂Z".as_bytes());
        let (snapshot, _) = s.render_snapshot(VecDeque::new());
        let plan = build_row(
            &snapshot.rows[0],
            Palette::new(ThemeMode::Dark),
            &snapshot.colors,
        );
        let runs: Vec<_> = plan
            .text
            .iter()
            .filter(|run| !run.ascii)
            .map(|run| (run.col, run.columns, run.text.as_str()))
            .collect();
        // xterm.js 6.0 default Unicode provider: VS15/VS16 do not add columns.
        assert_eq!(
            runs,
            [
                (0, 1, "⏸"),
                (2, 1, "⏸\u{fe0e}"),
                (4, 1, "⏸\u{fe0f}"),
                (6, 2, "🙂")
            ]
        );
        assert_eq!(snapshot.cursor.unwrap().col, 9);
    }

    #[test]
    fn ansi_colors_blank_backgrounds_and_hidden_text_are_preserved() {
        let s = Session::remote("%ansi-test".into(), 4, 40, Arc::new(|_| Ok(())));
        s.remote_output(
            b"\x1b]4;1;rgb:12/34/56\x07\x1b[31mA\x1b[7mB\x1b[0;44m   \x1b[0;8msecret\x1b[0;4m  ",
        );
        let (snap, _) = s.render_snapshot(VecDeque::new());
        let palette = Palette::new(ThemeMode::Dark);
        let plan = build_row(&snap.rows[0], palette, &snap.colors);
        assert_eq!(plan.text[0].fg, 0x123456);
        assert_eq!(plan.backgrounds[0].color, 0x123456);
        assert!(
            plan.backgrounds
                .iter()
                .any(|span| span.col == 2 && span.columns == 3 && span.color == palette.ansi[4])
        );
        assert!(!plan.text.iter().any(|r| r.text.contains("secret")));
        assert!(
            plan.text
                .iter()
                .any(|r| r.text == "  " && r.flags.contains(Flags::UNDERLINE))
        );
    }

    #[test]
    fn hit_testing_includes_half_cell_and_clamps_outside_viewport() {
        assert_eq!(
            hit_test(11., 0., 10., 20., 5, 10, 2),
            Anchor {
                point: Point::new(Line(-2), Column(1)),
                side: Side::Left
            }
        );
        assert_eq!(hit_test(16., 0., 10., 20., 5, 10, 2).side, Side::Right);
        assert_eq!(
            hit_test(-30., -30., 10., 20., 5, 10, 2).point,
            Point::new(Line(-2), Column(0))
        );
        assert_eq!(
            hit_test(900., 900., 10., 20., 5, 10, 2).point,
            Point::new(Line(2), Column(9))
        );
        assert!(selection_spans(Some(range((0, 0), (2, 3), false)), 0, 0, 0, 0).is_empty());
    }

    #[test]
    fn plain_ascii_is_batched_and_unused_spaces_create_no_text_work() {
        let cells = vec![
            Cell {
                c: 'x',
                ..Cell::default()
            };
            200
        ];
        let colors = Colors::default();
        let p = Palette::new(ThemeMode::Dark);
        assert_eq!(build_row(&cells, p, &colors).text.len(), 1);
        let blank = build_row(&vec![Cell::default(); 200], p, &colors);
        assert!(blank.text.is_empty());
        assert!(blank.backgrounds.is_empty());
    }

    fn range(start: (i32, usize), end: (i32, usize), is_block: bool) -> SelectionRange {
        SelectionRange {
            start: GridPoint::new(Line(start.0), Column(start.1)),
            end: GridPoint::new(Line(end.0), Column(end.1)),
            is_block,
        }
    }

    #[test]
    fn selection_and_background_spans() {
        let spans = selection_spans(Some(range((2, 3), (2, 6), false)), 5, 10, 0, 7);
        assert_eq!(
            spans,
            vec![PaintSpan {
                row: 2,
                col: 3,
                columns: 4,
                color: 7,
            }]
        );

        let spans = selection_spans(Some(range((-1, 3), (1, 5), false)), 4, 10, 2, 7);
        assert_eq!(
            spans,
            vec![
                PaintSpan {
                    row: 1,
                    col: 3,
                    columns: 7,
                    color: 7,
                },
                PaintSpan {
                    row: 2,
                    col: 0,
                    columns: 10,
                    color: 7,
                },
                PaintSpan {
                    row: 3,
                    col: 0,
                    columns: 6,
                    color: 7,
                },
            ]
        );

        let spans = selection_spans(Some(range((1, 2), (3, 4), true)), 5, 10, 0, 7);
        assert_eq!(spans.len(), 3);
        assert!(spans.iter().all(|span| span.col == 2 && span.columns == 3));

        let mut spans = Vec::new();
        push_span(
            &mut spans,
            PaintSpan {
                row: 1,
                col: 2,
                columns: 1,
                color: 7,
            },
        );
        push_span(
            &mut spans,
            PaintSpan {
                row: 1,
                col: 3,
                columns: 2,
                color: 7,
            },
        );
        assert_eq!(
            spans,
            vec![PaintSpan {
                row: 1,
                col: 2,
                columns: 3,
                color: 7,
            }]
        );
    }
}
