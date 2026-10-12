//! Bounded XTGETTCAP decoding. Only capabilities implemented by TShell are reported.

const QUERY_LIMIT: usize = 4096;
const REPLY_LIMIT: usize = 64;

enum Capability {
    Number(&'static str),
    Flag,
}

#[derive(Default)]
pub(super) struct CapabilityQuery {
    data: Option<Vec<u8>>,
}

impl CapabilityQuery {
    pub fn start(&mut self, enabled: bool) {
        self.data = enabled.then(Vec::new);
    }

    pub fn put(&mut self, byte: u8) {
        if let Some(data) = &mut self.data {
            if data.len() == QUERY_LIMIT {
                self.data = None;
            } else {
                data.push(byte);
            }
        }
    }

    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        let Some(data) = self.data.take() else {
            return Vec::new();
        };
        data.split(|&c| c == b';')
            .take(REPLY_LIMIT)
            .map(|name| {
                let value = hex::decode(name)
                    .ok()
                    .and_then(|decoded| match decoded.as_slice() {
                        b"Co" | b"colors" => Some(Capability::Number("256")),
                        b"RGB" | b"Tc" => Some(Capability::Flag),
                        _ => None,
                    });
                // Never echo unvalidated bytes back into the application's input stream.
                let name = if name.iter().all(u8::is_ascii_hexdigit) && name.len() % 2 == 0 {
                    String::from_utf8_lossy(name).into_owned()
                } else {
                    String::new()
                };
                match value {
                    Some(Capability::Number(value)) => {
                        format!("\x1bP1+r{name}={}\x1b\\", hex::encode(value))
                    }
                    Some(Capability::Flag) => format!("\x1bP1+r{name}\x1b\\"),
                    None => format!("\x1bP0+r{name}\x1b\\"),
                }
                .into_bytes()
            })
            .collect()
    }
}
