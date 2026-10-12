use super::*;
use crate::terminal_protocol::{ColorOverride, ExtraColors};
use crate::{
    terminal::Session,
    terminal_render::{build_selected_row, selected_colors},
};
use alacritty_terminal::{
    index::{Column, Line, Point, Side},
    selection::SelectionType,
    term::cell::{Cell, Flags},
    vte::ansi::{Color, NamedColor, Rgb},
};
use serde::Deserialize;
use std::collections::VecDeque;

fn session() -> Arc<Session> {
    Session::remote("%colour-protocol".into(), 2, 16, Arc::new(|_| Ok(())))
}

fn feed(session: &Session, bytes: &[u8], respond: bool) -> Vec<String> {
    session
        .remote_parser
        .lock()
        .advance(
            &mut session.term.lock(),
            bytes,
            &mut session.protocol.lock(),
            &session.replies,
            respond,
        )
        .replies
        .into_iter()
        .map(|r| String::from_utf8(r).unwrap())
        .collect()
}

#[derive(Deserialize)]
struct FixtureTheme {
    foreground: u32,
    background: u32,
    cursor: u32,
    selection: u32,
    ansi: [u32; 16],
}

impl From<FixtureTheme> for TerminalTheme {
    fn from(theme: FixtureTheme) -> Self {
        Self {
            foreground: theme.foreground,
            background: theme.background,
            cursor: theme.cursor,
            selection: theme.selection,
            ansi: theme.ansi,
        }
    }
}

#[derive(Deserialize)]
struct Fixture {
    theme: FixtureTheme,
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    commands: Vec<(u32, String)>,
    replies: Vec<String>,
}

#[test]
fn xterm_compat_color_protocol_matches_pinned_handlers_at_every_byte_split() {
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../tests/fixtures/xterm-color-protocol.json"
    ))
    .unwrap();
    let theme = TerminalTheme::from(fixture.theme);
    for case in fixture.cases {
        for terminator in ["\x07", "\x1b\\"] {
            let bytes = case
                .commands
                .iter()
                .map(|(code, value)| {
                    if value.is_empty() {
                        format!("\x1b]{code}{terminator}")
                    } else {
                        format!("\x1b]{code};{value}{terminator}")
                    }
                })
                .collect::<String>()
                .into_bytes();
            let expected = case
                .replies
                .iter()
                .map(|reply| reply.replace("\x1b\\", terminator))
                .collect::<Vec<_>>();
            for split in 0..=bytes.len() {
                let s = session();
                s.protocol.lock().theme = theme;
                let mut replies = feed(&s, &bytes[..split], true);
                replies.extend(feed(&s, &bytes[split..], true));
                assert_eq!(
                    replies, expected,
                    "commands={:?} split={split}",
                    case.commands
                );
            }
            let s = session();
            s.protocol.lock().theme = theme;
            let replies = bytes
                .iter()
                .flat_map(|byte| feed(&s, &[*byte], true))
                .collect::<Vec<_>>();
            assert_eq!(replies, expected, "bytewise {:?}", case.commands);
        }
    }
}

#[test]
fn color_protocol_named_queries_preserve_order_and_report_unknown_or_dynamic_fields() {
    for terminator in ["\x07", "\x1b\\"] {
        let bytes = format!(
            "\x1b]21;foreground=#123456;foreground=?;foreground=#abcdef;foreground=?;selection_foreground=?;selection_background=;selection_background=?;bogus=?;256=?{terminator}"
        );
        for split in 0..=bytes.len() {
            let s = session();
            let mut replies = feed(&s, &bytes.as_bytes()[..split], true);
            replies.extend(feed(&s, &bytes.as_bytes()[split..], true));
            assert_eq!(
                replies,
                [format!(
                    "\x1b]21;foreground=rgb:1212/3434/5656;foreground=rgb:abab/cdcd/efef;selection_foreground=;selection_background=;unknown=Ym9ndXM;unknown=MjU2{terminator}"
                )]
            );
            assert_eq!(
                s.protocol.lock().extra.selection_background,
                ColorOverride::Dynamic
            );
        }
    }
    let s = session();
    let theme = s.protocol.lock().theme;
    feed(
        &s,
        b"\x1b]21;1=green;selection_foreground=#112233;selection_background=#445566\x07",
        true,
    );
    let replies = feed(&s, b"\x1b]4;1;?\x07\x1b]17;?\x07\x1b]19;?\x07", true);
    assert_eq!(
        replies,
        [
            "\x1b]4;1;rgb:0000/8080/0000\x07",
            "\x1b]17;rgb:4444/5555/6666\x07",
            "\x1b]19;rgb:1111/2222/3333\x07"
        ]
    );
    feed(
        &s,
        b"\x1b]21;foreground;1;selection_foreground;selection_background\x07",
        true,
    );
    assert_eq!(s.protocol.lock().theme, theme);
    assert_eq!(s.protocol.lock().extra, ExtraColors::default());
    assert_eq!(s.term.lock().colors()[1], None);
}

#[test]
fn color_protocol_selection_and_cursor_reports_match_rendered_state_and_reset() {
    let s = session();
    let palette = crate::terminal_theme::default_palette();
    feed(
        &s,
        b"\x1b]17;#123456\x07\x1b]19;#abcdef\x07\x1b]12;#112233\x07",
        true,
    );
    let extra = s.protocol.lock().extra;
    let colors = *s.term.lock().colors();
    let mut cells = vec![Cell::default(); 4];
    for cell in &mut cells {
        cell.c = 'A';
    }
    cells[1].flags = Flags::BOLD | Flags::DIM | Flags::INVERSE;
    let row = build_selected_row(&cells, palette, &colors, Some((1, 3)), extra);
    assert_eq!(
        row.text.iter().find(|run| run.col == 1).unwrap().fg,
        0xabcdef
    );
    assert!(
        !row.text
            .iter()
            .find(|run| run.col == 1)
            .unwrap()
            .flags
            .contains(Flags::DIM)
    );
    assert_eq!(
        selected_colors(&cells[1], palette, &colors, extra),
        (0x123456, Some(0xabcdef))
    );
    assert_eq!(
        crate::terminal_render::resolve_color(Color::Named(NamedColor::Cursor), palette, &colors),
        0x112233
    );
    s.remote_output(b"AAAA");
    let before = s.render_snapshot(VecDeque::new()).0;
    feed(&s, b"\x1b]117\x07\x1b]119\x07\x1b]112\x07", true);
    let after = s.render_snapshot(VecDeque::new()).0;
    assert!(!before.same_visual(&after));
    assert_eq!(after.extra_colors, ExtraColors::default());
    assert_eq!(s.term.lock().colors()[NamedColor::Cursor as usize], None);
}

#[test]
fn color_protocol_dynamic_selection_swaps_displayed_cell_colors() {
    let palette = crate::terminal_theme::default_palette();
    let colors = alacritty_terminal::term::color::Colors::default();
    let cell = Cell {
        c: 'X',
        fg: Color::Indexed(1),
        bg: Color::Spec(Rgb { r: 1, g: 2, b: 3 }),
        flags: Flags::BOLD,
        ..Cell::default()
    };
    let extra = ExtraColors {
        selection_background: ColorOverride::Dynamic,
        selection_foreground: ColorOverride::Dynamic,
    };
    assert_eq!(
        selected_colors(&cell, palette, &colors, extra),
        (palette.ansi[9], Some(0x010203))
    );
    let row = build_selected_row(&[cell], palette, &colors, Some((0, 1)), extra);
    assert_eq!(row.text[0].fg, 0x010203);
}

#[test]
fn color_protocol_stacks_restore_actual_colors_across_theme_changes_and_share_syntaxes() {
    let s = session();
    let initial = s.protocol.lock().theme;
    assert_eq!(feed(&s, b"\x1b[#R", true), ["\x1b[?0;0#Q"]);
    feed(
        &s,
        b"\x1b]30001\x07\x1b]4;1;#112233\x07\x1b]17;#445566\x07\x1b[#P",
        true,
    );
    assert_eq!(feed(&s, b"\x1b[#R", true), ["\x1b[?2;2#Q"]);
    feed(&s, b"\x1b]4;1;#ffffff\x07\x1b]30101\x07", true);
    assert_eq!(
        feed(&s, b"\x1b]4;1;?\x07\x1b]17;?\x07", true),
        [
            "\x1b]4;1;rgb:1111/2222/3333\x07",
            "\x1b]17;rgb:4444/5555/6666\x07"
        ]
    );
    s.set_theme(TerminalTheme {
        foreground: 0xaabbcc,
        selection: 0xff00ff,
        ..initial
    });
    feed(&s, b"\x1b[#Q", true);
    assert_eq!(
        s.term.lock().colors()[NamedColor::Foreground as usize],
        Some(initial.rgb(NamedColor::Foreground as usize))
    );
    assert_eq!(
        s.protocol.lock().extra.selection_background,
        ColorOverride::Rgb(initial.selection)
    );
    assert_eq!(feed(&s, b"\x1b[#R", true), ["\x1b[?0;2#Q"]);
    // Saved slots remain available after a pop, as in xterm's palette stack.
    feed(&s, b"\x1b[10#P\x1b[10#Q", true);
    assert_eq!(feed(&s, b"\x1b[#R", true), ["\x1b[?9;10#Q"]);
    for _ in 0..20 {
        feed(&s, b"\x1b[#P", true);
    }
    assert_eq!(feed(&s, b"\x1b[#R", true), ["\x1b[?10;10#Q"]);
    feed(&s, b"\x1b[11#P\x1b[11#Q", true);
    assert_eq!(feed(&s, b"\x1b[#R", true), ["\x1b[?10;10#Q"]);
}

#[test]
fn color_protocol_capability_queries_handle_fragmentation_unknowns_and_cancellation() {
    let bytes = b"\x1bP+q436f;636f6c6f7273;524742;5463;626f677573\x1b\\";
    let expected = [
        "\x1bP1+r436f=323536\x1b\\",
        "\x1bP1+r636f6c6f7273=323536\x1b\\",
        "\x1bP1+r524742\x1b\\",
        "\x1bP1+r5463\x1b\\",
        "\x1bP0+r626f677573\x1b\\",
    ];
    for split in 0..=bytes.len() {
        let s = session();
        let mut replies = feed(&s, &bytes[..split], true);
        replies.extend(feed(&s, &bytes[split..], true));
        assert_eq!(replies, expected, "split={split}");
    }
    let s = session();
    for cancel in [0x18, 0x1a] {
        let mut bytes = b"\x1bP+q436f".to_vec();
        bytes.push(cancel);
        assert!(feed(&s, &bytes, true).is_empty());
    }
    let oversized = format!("\x1bP+q{}\x1b\\", "43".repeat(4097));
    assert!(feed(&s, oversized.as_bytes(), true).is_empty());
    assert_eq!(feed(&s, b"\x1bP+qzz\x1b\\", true), ["\x1bP0+r\x1b\\"]);
    let repeated = format!(
        "\x1bP+q{}\x1b\\",
        std::iter::repeat_n("436f", 100)
            .collect::<Vec<_>>()
            .join(";")
    );
    assert_eq!(feed(&s, repeated.as_bytes(), true).len(), 64);
}

#[test]
fn color_protocol_notifies_palette_changes_without_application_color_write_loops() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let sink = sent.clone();
    let s = Session::remote(
        "%theme-notify".into(),
        2,
        16,
        Arc::new(move |bytes| {
            sink.lock().push(bytes);
            Ok(())
        }),
    );
    let initial = s.protocol.lock().theme;
    feed(&s, b"\x1b[?2031h", true);
    let changed = TerminalTheme {
        foreground: initial.foreground ^ 0x000101,
        ..initial
    };
    assert_eq!(initial.light(), changed.light());
    s.set_theme(changed);
    s.set_theme(changed);
    assert_eq!(*sent.lock(), [changed.report()]);
    feed(&s, b"\x1b]11;#ffffff\x07\x1b]4;1;#123456\x07", true);
    assert_eq!(sent.lock().len(), 1);
    feed(&s, b"\x1b[?2031l", true);
    s.set_theme(initial);
    assert_eq!(sent.lock().len(), 1);
}

#[test]
fn color_protocol_tmux_streams_never_inject_replies_or_restore_synthetic_extension_state() {
    let s = session();
    let bytes =
        b"\x1b]4;1;?\x07\x1b]21;foreground=?\x07\x1b]17;?\x07\x1b[#R\x1bP+q436f\x1b\\\x1b[?996n";
    assert!(feed(&s, bytes, false).is_empty());
    feed(&s, b"\x1b]17;#123456\x07\x1b[?2031h", false);
    let extra = s.protocol.lock().extra;
    s.remote_restore(b"\x1bcsynthetic");
    assert_eq!(s.protocol.lock().extra, extra);
    assert!(s.protocol.lock().subscribed);
    feed(&s, b"\x1bc", false);
    assert_eq!(s.protocol.lock().extra, ExtraColors::default());
    assert!(!s.protocol.lock().subscribed);
}

#[test]
fn color_protocol_canceled_osc_and_independent_panes_do_not_share_overrides() {
    let first = session();
    let second = session();
    for cancel in [0x18, 0x1a] {
        let mut bytes = b"\x1b]21;selection_foreground=#123456".to_vec();
        bytes.push(cancel);
        assert!(feed(&first, &bytes, true).is_empty());
        assert_eq!(first.protocol.lock().extra, ExtraColors::default());
        let mut bytes = b"\x1b]4;1;#123456".to_vec();
        bytes.push(cancel);
        assert!(feed(&first, &bytes, true).is_empty());
        assert_eq!(first.term.lock().colors()[1], None);
    }
    feed(&first, b"\x1b]17;#123456\x07", true);
    assert_eq!(second.protocol.lock().extra, ExtraColors::default());
    first.remote_output(b"hello");
    first.render_snapshot(VecDeque::from([
        crate::terminal_snapshot::RenderCommand::Begin(
            crate::terminal_snapshot::Anchor {
                point: Point::new(Line(0), Column(0)),
                side: Side::Left,
            },
            SelectionType::Simple,
        ),
    ]));
}
