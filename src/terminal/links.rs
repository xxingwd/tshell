use super::*;
use alacritty_terminal::{
    index::{Column, Point},
    term::cell::Flags,
};
use std::{ops::Range, sync::OnceLock};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    Url(Url),
    File {
        path: String,
        host: Option<String>,
        line: Option<usize>,
        column: Option<usize>,
    },
}

#[derive(Clone)]
pub(crate) struct OpenLink {
    pub identity: u64,
    pub target: LinkTarget,
    pub directory: Option<String>,
}

pub(crate) fn parse_link(value: &str, explicit: bool) -> Option<LinkTarget> {
    if value.chars().any(char::is_control) || value.len() > 8192 {
        return None;
    }
    let (path, line, column) = file_location(value);
    let relative_source = line.is_some() && std::path::Path::new(&path).extension().is_some();
    if let Ok(url) = Url::parse(value) {
        if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() {
            return Some(LinkTarget::Url(url));
        }
        if url.scheme() == "file" {
            let path = percent_encoding::percent_decode_str(url.path())
                .decode_utf8()
                .ok()?
                .into_owned();
            let (path, line, column) = file_location(&path);
            return Some(LinkTarget::File {
                path,
                host: url.host_str().map(str::to_owned),
                line,
                column,
            });
        }
        // URL parsers treat a Windows drive letter as a scheme.
        if !is_windows_path(value) && (explicit || !relative_source) {
            return None;
        }
    }
    if !explicit && !(value.contains('/') || is_windows_path(value) || relative_source) {
        return None;
    }
    (!path.is_empty()).then_some(LinkTarget::File {
        path,
        host: None,
        line,
        column,
    })
}

fn is_windows_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

fn file_location(value: &str) -> (String, Option<usize>, Option<usize>) {
    fn suffix(value: &str) -> Option<(&str, usize)> {
        let (path, number) = value.rsplit_once(':')?;
        let number = number.parse::<usize>().ok().filter(|number| *number > 0)?;
        Some((path, number))
    }
    if let Some((path, last)) = suffix(value) {
        if let Some((path, line)) = suffix(path) {
            (path.to_owned(), Some(line), Some(last))
        } else {
            (path.to_owned(), Some(last), None)
        }
    } else {
        (value.to_owned(), None, None)
    }
}

fn trim_url(value: &str) -> &str {
    let mut value = value.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        while value.ends_with(close) && value.matches(close).count() > value.matches(open).count() {
            value = &value[..value.len() - close.len_utf8()];
        }
    }
    value
}

pub(crate) fn text_link(text: &str, byte: usize) -> Option<LinkTarget> {
    text_link_span(text, byte).map(|(target, _)| target)
}

fn text_link_span(text: &str, byte: usize) -> Option<(LinkTarget, Range<usize>)> {
    static URLS: OnceLock<regex::Regex> = OnceLock::new();
    let urls = URLS.get_or_init(|| {
        regex::Regex::new(r#"https?://[^\s<>\"'`]+"#).expect("static URL expression")
    });
    for found in urls.find_iter(text) {
        let value = trim_url(found.as_str());
        let range = found.start()..found.start() + value.len();
        if range.contains(&byte) {
            return parse_link(value, false).map(|target| (target, range));
        }
    }
    let start = text[..byte.min(text.len())]
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | '<'))
        .map_or(0, |(index, c)| index + c.len_utf8());
    let end = text[byte.min(text.len())..]
        .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ')' | '>'))
        .map_or(text.len(), |index| byte + index);
    let value = text.get(start..end)?.trim_end_matches([',', ';']);
    (byte < start + value.len())
        .then(|| parse_link(value, false).map(|target| (target, start..start + value.len())))
        .flatten()
}

pub(crate) fn hovered_cells(
    snapshot: &RenderSnapshot,
    row: usize,
    mut column: usize,
) -> Option<Vec<(usize, usize)>> {
    let cell = snapshot.rows.get(row)?.get(column)?;
    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) && column > 0 {
        column -= 1;
    }
    if let Some(link) = snapshot.rows[row][column].hyperlink() {
        parse_link(link.uri(), true)?;
        return Some(
            snapshot
                .rows
                .iter()
                .enumerate()
                .flat_map(|(row, cells)| {
                    cells
                        .iter()
                        .enumerate()
                        .filter_map(|(col, cell)| {
                            cell.hyperlink()
                                .filter(|candidate| candidate == &link)
                                .map(|_| (row, col))
                        })
                        .collect::<Vec<_>>()
                })
                .collect(),
        );
    }
    let mut start = row;
    while start > 0
        && snapshot.rows[start - 1]
            .last()
            .is_some_and(|cell| cell.flags.contains(Flags::WRAPLINE))
    {
        start -= 1;
    }
    let mut text = String::new();
    let mut cells = Vec::new();
    let mut byte = None;
    for index in start..snapshot.rows.len() {
        for (col, cell) in snapshot.rows[index].iter().enumerate() {
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let from = text.len();
            if index == row && col == column {
                byte = Some(from);
            }
            text.push(cell.c);
            if let Some(zero) = cell.zerowidth() {
                text.extend(zero.iter().copied());
            }
            cells.push((
                from..text.len(),
                index,
                col,
                cell.flags.contains(Flags::WIDE_CHAR),
            ));
        }
        if !snapshot.rows[index]
            .last()
            .is_some_and(|cell| cell.flags.contains(Flags::WRAPLINE))
            || text.len() > 32_768
        {
            break;
        }
    }
    let (_, range) = text_link_span(&text, byte?)?;
    Some(
        cells
            .into_iter()
            .filter(|(cell, _, _, _)| cell.start < range.end && cell.end > range.start)
            .flat_map(|(_, row, col, wide)| {
                if wide {
                    vec![(row, col), (row, col + 1)]
                } else {
                    vec![(row, col)]
                }
            })
            .collect(),
    )
}

impl Session {
    pub(crate) fn link_at(&self, mut point: Point, revision: u64) -> Option<OpenLink> {
        let term = self.term.lock();
        if self.revision.load(Ordering::Acquire) != revision
            || point.line < term.topmost_line()
            || point.line > term.bottommost_line()
            || point.column > term.last_column()
        {
            return None;
        }
        if term.grid()[point].flags.contains(Flags::WIDE_CHAR_SPACER) && point.column.0 > 0 {
            point.column -= 1;
        }
        let target = if let Some(link) = term.grid()[point].hyperlink() {
            parse_link(link.uri(), true)?
        } else {
            let mut start = point.line;
            while start > term.topmost_line()
                && term.grid()[start - 1i32][term.last_column()]
                    .flags
                    .contains(Flags::WRAPLINE)
                && point.line.0 - start.0 < 128
            {
                start -= 1;
            }
            let mut text = String::new();
            let mut byte = None;
            let mut line = start;
            'rows: loop {
                for column in 0..term.columns() {
                    let cell = &term.grid()[line][Column(column)];
                    if cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        continue;
                    }
                    if Point::new(line, Column(column)) == point {
                        byte = Some(text.len());
                    }
                    text.push(cell.c);
                    if let Some(zero) = cell.zerowidth() {
                        text.extend(zero.iter().copied());
                    }
                    if text.len() > 32_768 {
                        break 'rows;
                    }
                }
                if line >= term.bottommost_line()
                    || !term.grid()[line][term.last_column()]
                        .flags
                        .contains(Flags::WRAPLINE)
                {
                    break;
                }
                line += 1;
            }
            text_link(&text, byte?)?
        };
        drop(term);
        Some(OpenLink {
            identity: self.identity,
            target,
            directory: self.current_directory(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links_use_structured_urls_and_keep_windows_drive_and_source_location() {
        assert!(
            matches!(text_link("see (https://example.org/a?q=1).", 12), Some(LinkTarget::Url(url)) if url.as_str() == "https://example.org/a?q=1")
        );
        assert!(
            matches!(parse_link("C:\\work\\file.rs:12:3", false), Some(LinkTarget::File {path, line: Some(12), column: Some(3), ..}) if path == "C:\\work\\file.rs")
        );
        assert!(matches!(
            parse_link("/srv/app.rs:7", false),
            Some(LinkTarget::File { line: Some(7), .. })
        ));
        assert!(
            matches!(parse_link("src/main.rs:7:2", false), Some(LinkTarget::File {path, line: Some(7), column: Some(2), ..}) if path == "src/main.rs")
        );
        assert!(matches!(
            parse_link("main.rs:7", false),
            Some(LinkTarget::File { line: Some(7), .. })
        ));
        assert!(
            matches!(parse_link("file://server/home/a%20b.txt", true), Some(LinkTarget::File {path, host: Some(host), ..}) if path == "/home/a b.txt" && host == "server")
        );
        assert!(parse_link("javascript:alert(1)", true).is_none());
        assert!(parse_link("ssh://server", true).is_none());
    }
    #[test]
    fn osc_links_and_wrapped_urls_are_resolved_at_grid_cells() {
        let screen = Session::remote("links".into(), 5, 12, Arc::new(|_| Ok(())));
        screen.remote_output(
            b"\x1b]8;;https://example.org\x1b\\label\x1b]8;;\x1b\\\r\nhttps://example.org/path",
        );
        let revision = screen.revision.load(Ordering::Acquire);
        assert!(
            matches!(screen.link_at(Point::new(0.into(), Column(2)), revision).map(|link| link.target), Some(LinkTarget::Url(url)) if url.as_str() == "https://example.org/")
        );
        assert!(
            matches!(screen.link_at(Point::new(2.into(), Column(4)), revision).map(|link| link.target), Some(LinkTarget::Url(url)) if url.as_str() == "https://example.org/path")
        );
        screen.remote_output(b"x");
        assert!(
            screen
                .link_at(Point::new(0.into(), Column(2)), revision)
                .is_none()
        );
    }
}
