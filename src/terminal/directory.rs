//! OSC 7 directory metadata is a workspace extension, separate from screen rendering.
use alacritty_terminal::vte::{Parser, Perform};

#[derive(Default)]
pub(super) struct Directory {
    parser: Parser,
    pub path: Option<String>,
}
impl Directory {
    pub fn advance(&mut self, bytes: &[u8]) -> bool {
        struct Sink<'a>(&'a mut Option<String>);
        impl Perform for Sink<'_> {
            fn osc_dispatch(&mut self, params: &[&[u8]], _: bool) {
                if let [b"7", uri] = params {
                    if let Some(path) = decode(uri) {
                        *self.0 = Some(path);
                    }
                }
            }
        }
        let before = self.path.clone();
        for part in bytes.split_inclusive(|b| matches!(b, 0x18 | 0x1a)) {
            if matches!(part.last(), Some(0x18 | 0x1a)) {
                self.parser
                    .advance(&mut Sink(&mut self.path), &part[..part.len() - 1]);
                self.parser = Parser::default();
            } else {
                self.parser.advance(&mut Sink(&mut self.path), part);
            }
        }
        before != self.path
    }
}
fn decode(uri: &[u8]) -> Option<String> {
    let uri = std::str::from_utf8(uri).ok()?.strip_prefix("file://")?;
    let (_, path) = uri.split_once('/')?;
    let path = format!("/{path}");
    let mut out = Vec::new();
    let mut bytes = path.bytes();
    while let Some(b) = bytes.next() {
        out.push(if b == b'%' {
            let a = (bytes.next()? as char).to_digit(16)?;
            let b = (bytes.next()? as char).to_digit(16)?;
            (a * 16 + b) as u8
        } else {
            b
        });
    }
    let path = String::from_utf8(out).ok()?;
    if path.chars().any(char::is_control) {
        return None;
    }
    Some(if cfg!(windows) && path.as_bytes().get(2) == Some(&b':') {
        path[1..].into()
    } else {
        path
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_metadata_handles_fragments_and_cancellation() {
        let sequence = b"\x1b]7;file://host/home/my%20project\x07";
        for split in 0..sequence.len() {
            let mut tracker = Directory::default();
            tracker.advance(&sequence[..split]);
            tracker.advance(&sequence[split..]);
            assert_eq!(tracker.path.as_deref(), Some("/home/my project"));
            tracker.advance(b"\x1b]7;file://host/wrong\x18");
            assert_eq!(tracker.path.as_deref(), Some("/home/my project"));
        }
        assert!(decode(b"file://host/%00bad").is_none());
        assert!(decode(b"https://host/path").is_none());
    }
}
