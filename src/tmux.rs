//! Incremental tmux control-mode framing and authoritative pane geometry.
//! Raw control messages must never be fed to a terminal emulator.
use anyhow::{Result, bail};

#[derive(Debug, PartialEq, Eq)]
pub enum ControlEvent {
    Output { pane: String, bytes: Vec<u8> },
    Response { success: bool, lines: Vec<Vec<u8>> },
    Line(String),
}

#[derive(Default)]
pub struct ControlParser {
    pending: Vec<u8>,
    block: Option<(Vec<u8>, Vec<Vec<u8>>)>,
    block_bytes: usize,
}

impl ControlParser {
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<ControlEvent>> {
        if self.pending.len() + data.len() > 4 * 1024 * 1024 {
            self.pending.clear();
            bail!("tmux control line exceeds 4 MiB");
        }
        self.pending.extend_from_slice(data);
        // tmux -CC wraps control output in a DCS 1000 envelope. The payload
        // remains the same control protocol used by -C after the envelope is
        // removed. tmux escapes literal ESC bytes in pane output, so these
        // two raw sequences are safe framing markers here.
        while let Some(start) = self.pending.windows(7).position(|w| w == b"\x1bP1000p") {
            self.pending.drain(start..start + 7);
        }
        while let Some(end) = self.pending.windows(2).position(|w| w == b"\x1b\\") {
            self.pending.drain(end..end + 2);
        }
        let mut events = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let mut line: Vec<_> = self.pending.drain(..=end).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if let Some((identity, lines)) = self.block.as_mut() {
                let end = line
                    .strip_prefix(b"%end ")
                    .map(|id| (true, id))
                    .or_else(|| line.strip_prefix(b"%error ").map(|id| (false, id)));
                if let Some((success, id)) = end.filter(|(_, id)| *id == identity.as_slice()) {
                    let _ = id;
                    let (_, lines) = self.block.take().unwrap();
                    self.block_bytes = 0;
                    events.push(ControlEvent::Response { success, lines });
                } else {
                    self.block_bytes += line.len();
                    anyhow::ensure!(
                        self.block_bytes < 16 * 1024 * 1024,
                        "tmux response exceeds 16 MiB"
                    );
                    lines.push(line);
                }
                continue;
            }
            if let Some(identity) = line.strip_prefix(b"%begin ") {
                self.block = Some((identity.to_vec(), Vec::new()));
                self.block_bytes = 0;
            } else if let Some(output) = line.strip_prefix(b"%output ") {
                let split = output
                    .iter()
                    .position(|b| *b == b' ')
                    .ok_or_else(|| anyhow::anyhow!("invalid tmux output"))?;
                let pane = String::from_utf8(output[..split].to_vec())?;
                anyhow::ensure!(
                    pane.starts_with('%') && pane[1..].parse::<u64>().is_ok(),
                    "invalid pane id"
                );
                events.push(ControlEvent::Output {
                    pane,
                    bytes: unescape(&output[split + 1..])?,
                });
            } else {
                events.push(ControlEvent::Line(String::from_utf8(line)?));
            }
        }
        Ok(events)
    }
}

/// Parse tmux's classic layout format, including nested and zoomed layouts.
pub fn parse_layout(value: &str) -> Result<Vec<crate::backend::PaneInfo>> {
    use crate::backend::PaneInfo;
    fn number(bytes: &[u8], i: &mut usize) -> Result<usize> {
        let start = *i;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        anyhow::ensure!(*i > start, "layout expected a number");
        Ok(std::str::from_utf8(&bytes[start..*i])?.parse()?)
    }
    fn expect(bytes: &[u8], i: &mut usize, c: u8) -> Result<()> {
        anyhow::ensure!(bytes.get(*i) == Some(&c), "invalid tmux layout delimiter");
        *i += 1;
        Ok(())
    }
    fn node(bytes: &[u8], i: &mut usize, out: &mut Vec<PaneInfo>, depth: usize) -> Result<()> {
        anyhow::ensure!(depth < 64, "layout nesting limit");
        let cols = number(bytes, i)?;
        expect(bytes, i, b'x')?;
        let rows = number(bytes, i)?;
        expect(bytes, i, b',')?;
        let x = number(bytes, i)?;
        expect(bytes, i, b',')?;
        let y = number(bytes, i)?;
        anyhow::ensure!(
            cols > 0 && rows > 0 && cols <= 10000 && rows <= 10000,
            "invalid pane dimensions"
        );
        match bytes.get(*i).copied() {
            Some(b'{' | b'[') => {
                let closing = if bytes[*i] == b'{' { b'}' } else { b']' };
                *i += 1;
                loop {
                    node(bytes, i, out, depth + 1)?;
                    if bytes.get(*i) == Some(&closing) {
                        *i += 1;
                        break;
                    }
                    expect(bytes, i, b',')?;
                }
            }
            Some(b',') => {
                *i += 1;
                let id = number(bytes, i)?;
                out.push(PaneInfo {
                    id: format!("%{id}"),
                    x,
                    y,
                    cols,
                    rows,
                    ..Default::default()
                });
            }
            _ => bail!("invalid tmux layout node"),
        }
        Ok(())
    }
    let (_, tree) = value
        .split_once(',')
        .ok_or_else(|| anyhow::anyhow!("missing tmux layout checksum"))?;
    let mut i = 0;
    let mut out = Vec::new();
    node(tree.as_bytes(), &mut i, &mut out, 0)?;
    anyhow::ensure!(i == tree.len(), "trailing layout data");
    Ok(out)
}

fn unescape(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\\' {
            anyhow::ensure!(i + 3 < data.len(), "truncated tmux escape");
            let digits = &data[i + 1..i + 4];
            anyhow::ensure!(
                digits.iter().all(|b| (b'0'..=b'7').contains(b)) && digits[0] <= b'3',
                "invalid tmux octal escape"
            );
            out.push((digits[0] - b'0') * 64 + (digits[1] - b'0') * 8 + (digits[2] - b'0'));
            i += 4;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_output_preserves_binary_bytes() {
        let mut parser = ControlParser::default();
        assert!(parser.push(b"%output %7 hi\\0").unwrap().is_empty());
        assert_eq!(
            parser
                .push(b"33[31m\\377\r\n%layout-change @1 layout\n")
                .unwrap(),
            vec![
                ControlEvent::Output {
                    pane: "%7".into(),
                    bytes: b"hi\x1b[31m\xff".to_vec()
                },
                ControlEvent::Line("%layout-change @1 layout".into()),
            ]
        );
    }
    #[test]
    fn strips_tmux_cc_dcs_envelope_even_when_fragmented() {
        let mut parser = ControlParser::default();
        assert!(parser.push(b"\x1bP100").unwrap().is_empty());
        assert_eq!(
            parser.push(b"0p%output %7 hello\\012\n\x1b\\").unwrap(),
            vec![ControlEvent::Output {
                pane: "%7".into(),
                bytes: b"hello\n".to_vec(),
            }]
        );
    }
    #[test]
    fn rejects_malformed_escapes() {
        assert!(
            ControlParser::default()
                .push(b"%output %1 \\999\n")
                .is_err()
        );
    }
    #[test]
    fn response_content_is_not_mistaken_for_notifications() {
        let events = ControlParser::default()
            .push(b"%begin 12 3 1\n%output %7 literal text\n%end 12 3 1\n")
            .unwrap();
        assert_eq!(
            events,
            vec![ControlEvent::Response {
                success: true,
                lines: vec![b"%output %7 literal text".to_vec()]
            }]
        );
    }
    #[test]
    fn parses_nested_native_panes_and_rejects_incomplete_layouts() {
        let panes =
            parse_layout("abcd,120x40,0,0{59x40,0,0,1,60x40,60,0[60x19,60,0,2,60x20,60,20,3]}")
                .unwrap();
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[2].id, "%3");
        assert_eq!(
            (panes[2].x, panes[2].y, panes[2].cols, panes[2].rows),
            (60, 20, 60, 20)
        );
        assert!(parse_layout("abcd,80x24,0,0{").is_err());
    }
}
