//! Shared colour operations for traditional OSC and named Kitty colour control.
use super::ProtocolState;
use crate::terminal::Listener;
use alacritty_terminal::{
    term::{
        Term,
        color::{COUNT, Colors},
    },
    vte::ansi::{Handler, NamedColor, Rgb},
};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};

const PALETTE_COLOR_COUNT: usize = NamedColor::Cursor as usize + 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ColorOverride {
    #[default]
    Theme,
    Dynamic,
    Rgb(u32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ExtraColors {
    pub selection_background: ColorOverride,
    pub selection_foreground: ColorOverride,
}

#[derive(Clone)]
struct SavedColors {
    colors: [Rgb; PALETTE_COLOR_COUNT],
    extra: ExtraColors,
}

#[derive(Default)]
pub(super) struct ColorStack {
    slots: [Option<SavedColors>; 10],
    current: usize,
    highest: usize,
}

#[derive(Clone, Copy)]
enum ColorSlot {
    Terminal(usize),
    SelectionBackground,
    SelectionForeground,
}

impl ColorSlot {
    fn named(key: &[u8]) -> Option<Self> {
        match key {
            b"foreground" => Some(Self::Terminal(NamedColor::Foreground as usize)),
            b"background" => Some(Self::Terminal(NamedColor::Background as usize)),
            b"cursor" => Some(Self::Terminal(NamedColor::Cursor as usize)),
            b"selection_background" => Some(Self::SelectionBackground),
            b"selection_foreground" => Some(Self::SelectionForeground),
            _ => number(key).filter(|&i| i < 256).map(Self::Terminal),
        }
    }

    fn osc(code: usize) -> Option<Self> {
        match code {
            10..=12 => Some(Self::Terminal(NamedColor::Foreground as usize + code - 10)),
            17 => Some(Self::SelectionBackground),
            19 => Some(Self::SelectionForeground),
            _ => None,
        }
    }

    fn rgb(self, term: &Term<Listener>, state: &ProtocolState) -> Option<u32> {
        match self {
            Self::Terminal(index) => Some(pack(
                term.colors()[index].unwrap_or_else(|| state.theme.rgb(index)),
            )),
            Self::SelectionBackground => match state.extra.selection_background {
                ColorOverride::Theme => Some(state.theme.selection),
                ColorOverride::Dynamic => None,
                ColorOverride::Rgb(rgb) => Some(rgb),
            },
            Self::SelectionForeground => match state.extra.selection_foreground {
                ColorOverride::Theme | ColorOverride::Dynamic => None,
                ColorOverride::Rgb(rgb) => Some(rgb),
            },
        }
    }

    fn set(self, term: &mut Term<Listener>, state: &mut ProtocolState, value: ColorOverride) {
        match self {
            Self::Terminal(index) => match value {
                ColorOverride::Theme => term.reset_color(index),
                ColorOverride::Rgb(rgb) => term.set_color(index, unpack(rgb)),
                // Default foreground/background/cursor are explicitly configured in TShell.
                ColorOverride::Dynamic => {}
            },
            Self::SelectionBackground => state.extra.selection_background = value,
            Self::SelectionForeground => state.extra.selection_foreground = value,
        }
    }
}

pub(super) fn handles_osc(code: &[u8]) -> bool {
    matches!(
        code,
        b"4" | b"10"
            | b"11"
            | b"12"
            | b"17"
            | b"19"
            | b"21"
            | b"104"
            | b"110"
            | b"111"
            | b"112"
            | b"117"
            | b"119"
            | b"30001"
            | b"30101"
    )
}

pub(super) fn osc(
    params: &[Vec<u8>],
    terminator: &str,
    term: &mut Term<Listener>,
    state: &mut ProtocolState,
) -> Vec<Vec<u8>> {
    let mut replies = Vec::new();
    let Some(code) = params.first().and_then(|p| number(p)) else {
        return replies;
    };
    match code {
        4 => {
            for pair in params[1..].chunks_exact(2) {
                if let Some(index) = number(&pair[0]).filter(|&i| i < 256) {
                    traditional(
                        ColorSlot::Terminal(index),
                        &format!("4;{index}"),
                        &pair[1],
                        terminator,
                        term,
                        state,
                        &mut replies,
                    );
                }
            }
        }
        10..=12 | 17 | 19 => {
            for (offset, value) in params[1..].iter().enumerate() {
                let Some(slot) = ColorSlot::osc(code + offset) else {
                    break;
                };
                traditional(
                    slot,
                    &(code + offset).to_string(),
                    value,
                    terminator,
                    term,
                    state,
                    &mut replies,
                );
            }
        }
        21 => {
            let mut fields = Vec::new();
            for field in &params[1..] {
                let (key, value) = match field.iter().position(|&c| c == b'=') {
                    Some(i) => (&field[..i], Some(&field[i + 1..])),
                    None => (field.as_slice(), None),
                };
                let Some(slot) = ColorSlot::named(key) else {
                    if value == Some(b"?".as_slice()) {
                        fields.push(format!("unknown={}", STANDARD_NO_PAD.encode(key)));
                    }
                    continue;
                };
                match value {
                    Some(b"?") => {
                        let value = slot.rgb(term, state).map(rgb_string).unwrap_or_default();
                        fields.push(format!("{}={value}", String::from_utf8_lossy(key)));
                    }
                    None => slot.set(term, state, ColorOverride::Theme),
                    Some(b"") => slot.set(term, state, ColorOverride::Dynamic),
                    Some(value) => {
                        if let Some(rgb) = parse_color(value, true) {
                            slot.set(term, state, ColorOverride::Rgb(rgb));
                        }
                    }
                }
            }
            if !fields.is_empty() {
                replies.push(format!("\x1b]21;{}{}", fields.join(";"), terminator).into_bytes());
            }
        }
        104 => {
            if params.len() == 1 || (params.len() == 2 && params[1].is_empty()) {
                for index in 0..256 {
                    term.reset_color(index);
                }
            } else {
                for index in params[1..]
                    .iter()
                    .filter_map(|p| number(p))
                    .filter(|&i| i < 256)
                {
                    term.reset_color(index);
                }
            }
        }
        110..=112 | 117 | 119 => {
            if let Some(slot) = ColorSlot::osc(code - 100) {
                slot.set(term, state, ColorOverride::Theme);
            }
        }
        30001 => push(0, term, state),
        30101 => pop(0, term, state),
        _ => {}
    }
    replies
}

fn traditional(
    slot: ColorSlot,
    prefix: &str,
    value: &[u8],
    terminator: &str,
    term: &mut Term<Listener>,
    state: &mut ProtocolState,
    replies: &mut Vec<Vec<u8>>,
) {
    if value == b"?" {
        // Traditional OSC has no representation for an unset/dynamic colour.
        let fallback = if matches!(slot, ColorSlot::SelectionBackground) {
            state.theme.background
        } else {
            state.theme.foreground
        };
        let rgb = slot.rgb(term, state).unwrap_or(fallback);
        replies.push(format!("\x1b]{prefix};{}{terminator}", rgb_string(rgb)).into_bytes());
    } else if let Some(rgb) = parse_color(value, false) {
        slot.set(term, state, ColorOverride::Rgb(rgb));
    }
}

pub(super) fn restore_overrides(term: &mut Term<Listener>, colors: &Colors) {
    for index in 0..COUNT {
        if term.colors()[index] != colors[index] {
            match colors[index] {
                Some(rgb) => term.set_color(index, rgb),
                None => term.reset_color(index),
            }
        }
    }
}

pub(super) fn push(slot: usize, term: &Term<Listener>, state: &mut ProtocolState) {
    let index = if slot == 0 {
        state.stack.current
    } else {
        slot - 1
    };
    if index >= state.stack.slots.len() {
        return;
    }
    let mut extra = state.extra;
    if extra.selection_background == ColorOverride::Theme {
        extra.selection_background = ColorOverride::Rgb(state.theme.selection);
    }
    state.stack.slots[index] = Some(SavedColors {
        colors: std::array::from_fn(|i| term.colors()[i].unwrap_or_else(|| state.theme.rgb(i))),
        extra,
    });
    state.stack.current = index + 1;
    state.stack.highest = state.stack.highest.max(index + 1);
}

pub(super) fn pop(slot: usize, term: &mut Term<Listener>, state: &mut ProtocolState) {
    let index = if slot == 0 {
        state.stack.current.checked_sub(1)
    } else {
        Some(slot - 1)
    };
    let Some(index) = index.filter(|&i| i < state.stack.slots.len()) else {
        return;
    };
    if let Some(saved) = &state.stack.slots[index] {
        for (index, &rgb) in saved.colors.iter().enumerate() {
            term.set_color(index, rgb);
        }
        state.extra = saved.extra;
        state.stack.current = index;
    }
}

pub(super) fn report_stack(state: &ProtocolState) -> Vec<u8> {
    format!("\x1b[?{};{}#Q", state.stack.current, state.stack.highest).into_bytes()
}

pub(super) fn number(value: &[u8]) -> Option<usize> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

pub(crate) fn pack(rgb: Rgb) -> u32 {
    (u32::from(rgb.r) << 16) | (u32::from(rgb.g) << 8) | u32::from(rgb.b)
}

fn unpack(rgb: u32) -> Rgb {
    Rgb {
        r: (rgb >> 16) as u8,
        g: (rgb >> 8) as u8,
        b: rgb as u8,
    }
}

fn rgb_string(rgb: u32) -> String {
    let rgb = unpack(rgb);
    format!(
        "rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}",
        rgb.r, rgb.g, rgb.b
    )
}

fn parse_color(value: &[u8], named: bool) -> Option<u32> {
    let text = std::str::from_utf8(value).ok()?.to_ascii_lowercase();
    let channel = |hex: &str| {
        if !(1..=4).contains(&hex.len()) || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()
    };
    if let Some(hex) = text.strip_prefix('#') {
        if !matches!(hex.len(), 3 | 6 | 9 | 12) {
            return None;
        }
        let width = hex.len() / 3;
        let mut rgb = 0;
        for chunk in hex.as_bytes().chunks_exact(width) {
            let value = channel(std::str::from_utf8(chunk).ok()?)?;
            let value = if width == 1 {
                value << 4
            } else {
                value >> ((width - 2) * 4)
            };
            rgb = (rgb << 8) | value;
        }
        return Some(rgb);
    }
    if let Some(spec) = text.strip_prefix("rgb:") {
        let channels = spec.split('/').collect::<Vec<_>>();
        if channels.len() != 3 {
            return None;
        }
        if !named && !channels.iter().all(|hex| hex.len() == channels[0].len()) {
            return None;
        }
        let mut rgb = 0;
        for hex in channels {
            let value = channel(hex)?;
            let maximum = (1u32 << (hex.len() * 4)) - 1;
            rgb = (rgb << 8) | ((value * 255 + maximum / 2) / maximum);
        }
        return Some(rgb);
    }
    if named {
        return match text.as_str() {
            "black" => Some(0x000000),
            "white" => Some(0xffffff),
            "red" => Some(0xff0000),
            "green" => Some(0x008000),
            "blue" => Some(0x0000ff),
            "yellow" => Some(0xffff00),
            "cyan" | "aqua" => Some(0x00ffff),
            "magenta" | "fuchsia" => Some(0xff00ff),
            "gray" | "grey" => Some(0x808080),
            "silver" => Some(0xc0c0c0),
            "maroon" => Some(0x800000),
            "olive" => Some(0x808000),
            "lime" => Some(0x00ff00),
            "teal" => Some(0x008080),
            "navy" => Some(0x000080),
            "purple" => Some(0x800080),
            _ => None,
        };
    }
    None
}
