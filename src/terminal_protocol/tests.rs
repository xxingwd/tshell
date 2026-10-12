use super::*;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
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
    ansi: [u32; 16],
}

impl From<FixtureTheme> for TerminalTheme {
    fn from(theme: FixtureTheme) -> Self {
        Self {
            foreground: theme.foreground,
            background: theme.background,
            cursor: theme.cursor,
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
fn color_protocol_cursor_query_matches_rendered_color_and_reset() {
    let s = session();
    let palette = crate::terminal_theme::default_palette();
    assert_eq!(
        feed(&s, b"\x1b]12;#112233\x07\x1b]12;?\x07", true),
        ["\x1b]12;rgb:1111/2222/3333\x07"]
    );
    let before = s.render_snapshot(VecDeque::new()).0;
    assert_eq!(
        crate::terminal_render::resolve_color(
            Color::Named(NamedColor::Cursor),
            palette,
            &before.colors
        ),
        0x112233
    );
    feed(&s, b"\x1b]112\x07", true);
    let after = s.render_snapshot(VecDeque::new()).0;
    assert!(!before.same_visual(&after));
    assert_eq!(
        crate::terminal_render::resolve_color(
            Color::Named(NamedColor::Cursor),
            palette,
            &after.colors
        ),
        palette.accent
    );
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
fn color_protocol_tmux_streams_do_not_reply_and_synthetic_reset_preserves_subscription() {
    let s = session();
    assert!(
        feed(
            &s,
            b"\x1b]4;1;?\x07\x1b]10;?;?;?\x07\x1b[?996n\x1b[?2031$p",
            false
        )
        .is_empty()
    );
    feed(&s, b"\x1b[?2031h", false);
    s.remote_restore(b"\x1bcsynthetic");
    assert!(s.protocol.lock().subscribed);
    feed(&s, b"\x1bc", false);
    assert!(!s.protocol.lock().subscribed);
}

#[test]
fn color_protocol_canceled_osc_and_independent_panes_do_not_share_overrides() {
    let first = session();
    let second = session();
    for cancel in [0x18, 0x1a] {
        for command in [b"\x1b]4;1;#123456".as_slice(), b"\x1b]10;#123456"] {
            let mut bytes = command.to_vec();
            bytes.push(cancel);
            for split in 0..=bytes.len() {
                assert!(feed(&first, &bytes[..split], true).is_empty());
                assert!(feed(&first, &bytes[split..], true).is_empty());
                assert_eq!(first.term.lock().colors()[1], None);
                assert_eq!(
                    first.term.lock().colors()[NamedColor::Foreground as usize],
                    None
                );
            }
        }
    }
    feed(&first, b"\x1b]4;1;#123456\x07", true);
    assert_ne!(first.term.lock().colors()[1], None);
    assert_eq!(second.term.lock().colors()[1], None);
}
