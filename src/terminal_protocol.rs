//! Terminal query replies and opt-in colour-scheme notifications.
mod colors;
use crate::appearance::PaletteTerminalTheme;
use crate::terminal::Listener;
use alacritty_terminal::{
    event::Event,
    term::Term,
    vte::{
        self, Params, Perform,
        ansi::{NamedColor, Processor, Rgb},
    },
};
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::{Arc, OnceLock},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalTheme {
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    pub ansi: [u32; 16],
}

pub(crate) fn indexed_color(index: u8, ansi: &[u32; 16]) -> u32 {
    match index {
        0..=15 => ansi[index as usize],
        16..=231 => {
            let i = index as u32 - 16;
            let level = |v| if v == 0 { 0 } else { 55 + v * 40 };
            (level(i / 36) << 16) | (level(i / 6 % 6) << 8) | level(i % 6)
        }
        232..=255 => (8 + (index as u32 - 232) * 10) * 0x010101,
    }
}

impl TerminalTheme {
    pub fn light(self) -> bool {
        let c = self.background;
        (c >> 16) * 299 + ((c >> 8) & 255) * 587 + (c & 255) * 114 >= 128000
    }
    pub fn report(self) -> Vec<u8> {
        format!("\x1b[?997;{}n", if self.light() { 2 } else { 1 }).into_bytes()
    }
    pub fn rgb(self, index: usize) -> Rgb {
        let value = match index {
            0..=255 => indexed_color(index as u8, &self.ansi),
            i if i == NamedColor::Background as usize => self.background,
            i if i == NamedColor::Cursor as usize => self.cursor,
            _ => self.foreground,
        };
        Rgb {
            r: (value >> 16) as u8,
            g: (value >> 8) as u8,
            b: value as u8,
        }
    }
}
fn theme_slot() -> &'static Mutex<TerminalTheme> {
    static THEME: OnceLock<Mutex<TerminalTheme>> = OnceLock::new();
    THEME.get_or_init(|| Mutex::new(crate::terminal_theme::default_palette().terminal_theme()))
}
pub fn default_theme() -> TerminalTheme {
    *theme_slot().lock()
}
pub fn set_default_theme(theme: TerminalTheme) {
    *theme_slot().lock() = theme;
}
pub struct ProtocolState {
    pub theme: TerminalTheme,
    pub subscribed: bool,
}
impl Default for ProtocolState {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            subscribed: false,
        }
    }
}
pub type Replies = Arc<Mutex<VecDeque<Event>>>;
struct Probe<'a> {
    state: &'a mut ProtocolState,
    boundary: bool,
    visual: bool,
    replies: Vec<Vec<u8>>,
    command: Option<ColorCommand>,
}
enum ColorCommand {
    Canceled,
    Osc(Vec<Vec<u8>>, &'static str),
}
impl Perform for Probe<'_> {
    fn print(&mut self, _: char) {
        self.visual = true;
    }
    fn execute(&mut self, byte: u8) {
        self.visual = true;
        if matches!(byte, 0x18 | 0x1a) {
            self.replies.clear();
            if self.command.is_some() {
                self.command = Some(ColorCommand::Canceled);
            }
        }
    }
    fn hook(&mut self, _: &Params, _: &[u8], _: bool, _: char) {
        self.visual = true;
    }
    fn put(&mut self, _: u8) {
        self.visual = true;
    }
    fn unhook(&mut self) {
        self.visual = true;
    }
    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        self.visual |= !matches!(params.first(), Some(&b"0" | &b"2"));
        // Stop at queries: set/query/reset in one read must report the value at
        // the query, rather than the final state of the read.
        self.boundary = params.contains(&b"?".as_slice());
        if params.first().is_some_and(|code| colors::handles_osc(code)) {
            self.boundary = true;
            self.command = Some(ColorCommand::Osc(
                params.iter().map(|p| p.to_vec()).collect(),
                if bell_terminated { "\x07" } else { "\x1b\\" },
            ));
        }
    }
    fn csi_dispatch(&mut self, params: &Params, intermediate: &[u8], ignore: bool, action: char) {
        self.visual = true;
        if ignore {
            return;
        }
        self.boundary = true;
        let mut values = params.iter();
        let first = values.next();
        if intermediate == b"?" && matches!(action, 'h' | 'l') {
            if params.iter().any(|p| p == [2031]) {
                self.state.subscribed = action == 'h';
            }
        } else if intermediate == b"?"
            && action == 'n'
            && first == Some(&[996][..])
            && values.next().is_none()
        {
            self.replies.push(self.state.theme.report());
        } else if intermediate == b"?$"
            && action == 'p'
            && first == Some(&[2031][..])
            && values.next().is_none()
        {
            self.replies.push(
                format!("\x1b[?2031;{}$y", if self.state.subscribed { 1 } else { 2 }).into_bytes(),
            );
        }
    }
    fn esc_dispatch(&mut self, intermediate: &[u8], ignore: bool, byte: u8) {
        // VTE dispatches the ST (ESC \) after an OSC separately; the
        // terminator itself does not change the terminal grid.
        self.visual |= !(intermediate.is_empty() && byte == b'\\');
        if !ignore && intermediate.is_empty() && byte == b'c' {
            self.state.subscribed = false;
        }
        self.boundary = true;
    }
    fn terminated(&self) -> bool {
        self.boundary
    }
}
#[derive(Default)]
pub struct OutputParser {
    terminal: Processor,
    probe: vte::Parser,
}
impl OutputParser {
    /// Live output can arrive while tmux is restoring a captured grid. Keep its
    /// subscriptions even when the corresponding screen bytes are discarded.
    pub fn observe(&mut self, bytes: &[u8], state: &mut ProtocolState) {
        let mut offset = 0;
        while offset < bytes.len() {
            let mut probe = Probe {
                state,
                boundary: false,
                visual: false,
                replies: Vec::new(),
                command: None,
            };
            offset += self
                .probe
                .advance_until_terminated(&mut probe, &bytes[offset..]);
        }
    }
    pub fn advance(
        &mut self,
        term: &mut Term<Listener>,
        bytes: &[u8],
        state: &mut ProtocolState,
        events: &Replies,
        respond: bool,
    ) -> ParsedOutput {
        let mut output = ParsedOutput::default();
        let mut offset = 0;
        while offset < bytes.len() {
            let mut probe = Probe {
                state,
                boundary: false,
                visual: false,
                replies: Vec::new(),
                command: None,
            };
            let count = self
                .probe
                .advance_until_terminated(&mut probe, &bytes[offset..]);
            let previous_colors = probe.command.as_ref().map(|_| *term.colors());
            self.terminal.advance(term, &bytes[offset..offset + count]);
            offset += count;
            output.visual |= probe.visual;
            if let Some(command) = probe.command {
                // Alacritty accepts these OSCs too. Replay against the colours at
                // the sequence boundary so set/query pairs inside one OSC stay ordered.
                if let Some(previous) = previous_colors {
                    colors::restore_overrides(term, &previous);
                }
                let replies = match command {
                    ColorCommand::Canceled => Vec::new(),
                    ColorCommand::Osc(params, terminator) => {
                        colors::osc(&params, terminator, term, probe.state.theme)
                    }
                };
                if respond {
                    output.replies.extend(replies);
                }
            }
            for event in events.lock().drain(..) {
                if !respond {
                    continue;
                } // tmux answers queries in control-mode panes.
                match event {
                    // Colour queries were answered in sequence order above.
                    Event::ColorRequest(_, _) => {}
                    Event::PtyWrite(text) if text != "\x1b[?2031;0$y" => {
                        output.replies.push(text.into_bytes())
                    }
                    _ => {}
                }
            }
            if respond {
                output.replies.extend(probe.replies);
            }
        }
        output
    }
}

#[derive(Default)]
pub struct ParsedOutput {
    pub replies: Vec<Vec<u8>>,
    pub visual: bool,
}
