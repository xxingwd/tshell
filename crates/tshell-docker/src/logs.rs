use std::collections::VecDeque;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINES: usize = 10_000;
#[derive(Default)]
pub struct LogBuffer {
    bytes: VecDeque<u8>,
    lines: usize,
}
impl LogBuffer {
    pub fn push(&mut self, chunk: &[u8]) {
        let chunk = if chunk.len() > MAX_BYTES {
            self.bytes.clear();
            self.lines = 0;
            &chunk[chunk.len() - MAX_BYTES..]
        } else {
            chunk
        };
        self.lines += chunk.iter().filter(|&&byte| byte == b'\n').count();
        self.bytes.extend(chunk);
        while self.bytes.len() > MAX_BYTES || self.lines > MAX_LINES {
            if self.bytes.pop_front() == Some(b'\n') {
                self.lines = self.lines.saturating_sub(1);
            }
        }
        while self.bytes.front().is_some_and(|byte| byte & 0xc0 == 0x80) {
            self.bytes.pop_front();
        }
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes.iter().copied().collect::<Vec<_>>()).into_owned()
    }
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.lines = 0;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_utf8_and_oversized_lines_preserve_bounded_valid_text() {
        let mut log = LogBuffer::default();
        let bytes = "中文\n".as_bytes();
        log.push(&bytes[..2]);
        log.push(&bytes[2..]);
        assert_eq!(log.text(), "中文\n");
        log.push(&"中文".repeat(MAX_BYTES).into_bytes());
        assert!(log.bytes.len() <= MAX_BYTES);
        assert!(!log.text().contains('�'));
    }
    #[test]
    fn old_lines_are_evicted() {
        let mut log = LogBuffer::default();
        for _ in 0..MAX_LINES + 25 {
            log.push(b"line\n");
        }
        assert_eq!(log.text().lines().count(), MAX_LINES);
    }
}
