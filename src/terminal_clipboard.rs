//! OSC 52 writes from live output; clipboard access stays on the UI thread.
use alacritty_terminal::vte::{Parser, Perform};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::OnceLock;

const MAX_TEXT_BYTES: usize = 1024 * 1024;
static WRITES: OnceLock<async_channel::Sender<String>> = OnceLock::new();

pub(crate) fn init(cx: &mut gpui::App) {
    let (sender, receiver) = async_channel::bounded(16);
    if WRITES.set(sender).is_err() {
        return;
    }
    cx.spawn(async move |cx| {
        while let Ok(text) = receiver.recv().await {
            cx.update(|cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)));
        }
    })
    .detach();
}

fn copy_text(params: &[&[u8]]) -> Option<String> {
    let [b"52", selection, data, ..] = params else {
        return None;
    };
    // Empty selection defaults to the system clipboard. Primary selections
    // remain separate; queries retain the existing copy-only policy.
    if !matches!(*selection, b"c" | b"") || *data == b"?" {
        return None;
    }
    if data.len() > MAX_TEXT_BYTES.div_ceil(3) * 4 {
        return None;
    }
    // The pinned xterm clipboard addon clears invalid/noncanonical text.
    let text = STANDARD
        .decode(data)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|text| STANDARD.encode(text).as_bytes() == *data)
        .unwrap_or_default();
    (text.len() <= MAX_TEXT_BYTES).then_some(text)
}

#[derive(Default)]
pub(crate) struct Clipboard(Parser);

impl Clipboard {
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parse(bytes, |text| {
            if let Some(sender) = WRITES.get() {
                let _ = sender.try_send(text);
            }
        });
    }

    fn parse(&mut self, bytes: &[u8], emit: impl FnMut(String)) {
        struct Sink<F>(F);
        impl<F: FnMut(String)> Perform for Sink<F> {
            fn osc_dispatch(&mut self, params: &[&[u8]], _: bool) {
                if let Some(text) = copy_text(params) {
                    (self.0)(text);
                }
            }
        }
        let mut sink = Sink(emit);
        for part in bytes.split_inclusive(|b| matches!(b, 0x18 | 0x1a)) {
            if matches!(part.last(), Some(0x18 | 0x1a)) {
                self.0.advance(&mut sink, &part[..part.len() - 1]);
                self.0 = Parser::default();
            } else {
                self.0.advance(&mut sink, part);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_matches_xterm_clipboard_writes_at_every_split() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/xterm-clipboard.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            for end in ["\x07", "\x1b\\"] {
                let bytes = format!("\x1b]52;{}{end}", case["payload"].as_str().unwrap());
                for split in 0..=bytes.len() {
                    let mut parser = Clipboard::default();
                    let mut writes = Vec::new();
                    parser.parse(&bytes.as_bytes()[..split], |s| writes.push(s));
                    parser.parse(&bytes.as_bytes()[split..], |s| writes.push(s));
                    assert_eq!(serde_json::json!(writes), case["writes"], "{case}, {split}");
                }
            }
        }
    }

    #[test]
    fn osc52_copy_only_defaults_cancellation_and_limits() {
        let mut parser = Clipboard::default();
        let mut writes = Vec::new();
        for bytes in [
            b"\x1b]52;;aGk=\x07".as_slice(),
            b"\x1b]52;c;?\x07",
            b"\x1b]52;c;aGk=\x18",
            b"\x1b]52;c;aGk=\x1a",
            b"\x1b]52;c;b2s=\x07",
        ] {
            parser.parse(bytes, |s| writes.push(s));
        }
        assert_eq!(writes, ["hi", "ok"]);
        let large = "x".repeat(MAX_TEXT_BYTES);
        assert_eq!(
            copy_text(&[b"52", b"c", STANDARD.encode(&large).as_bytes()]),
            Some(large)
        );
        assert_eq!(
            copy_text(&[
                b"52",
                b"c",
                STANDARD.encode("x".repeat(MAX_TEXT_BYTES + 1)).as_bytes()
            ]),
            None
        );
    }

    #[test]
    fn osc52_tmux_live_output_copies_but_capture_restoration_does_not() {
        let (sender, receiver) = async_channel::bounded(16);
        assert!(WRITES.set(sender).is_ok());
        let mut titles = crate::tmux_titles::LiveTitles::default();
        let mut snapshot = crate::backend::Snapshot::default();
        titles.advance("%1", b"\x1b]52;c;", &mut snapshot);
        titles.advance("%2", b"\x1b]52;c;dHdv\x07", &mut snapshot);
        titles.advance("%1", b"b25l\x07", &mut snapshot);
        assert_eq!(receiver.try_recv().unwrap(), "two");
        assert_eq!(receiver.try_recv().unwrap(), "one");
        let screen =
            crate::terminal::Session::remote("%1".into(), 24, 80, std::sync::Arc::new(|_| Ok(())));
        screen.remote_restore(b"\x1b]52;c;aGlzdG9yeQ==\x07");
        screen.remote_output(b"\x1b]52;c;b25l\x07");
        assert!(
            receiver.try_recv().is_err(),
            "rendering must not duplicate live writes"
        );
    }
}
