use super::*;
use alacritty_terminal::{
    index::{Column, Direction, Point},
    term::search::{Match, RegexIter, RegexSearch},
};

pub(crate) const MAX_MATCHES: usize = 10_000;

#[derive(Clone, Default)]
pub(crate) struct SearchQuery {
    pub text: String,
    pub case_sensitive: bool,
    pub regex: bool,
}

#[derive(Default)]
pub(crate) struct SearchResult {
    pub revision: u64,
    pub content_revision: u64,
    pub matches: Vec<Match>,
    pub current: Option<usize>,
    pub truncated: bool,
    pub invalid: bool,
}

impl Session {
    pub(crate) fn search_output(
        &self,
        query: &SearchQuery,
        current: Option<(Point, u64)>,
        navigation: Option<i8>,
    ) -> SearchResult {
        if query.text.is_empty() {
            return SearchResult::default();
        }
        let pattern = if query.regex {
            query.text.clone()
        } else {
            regex::escape(&query.text)
        };
        // Override the engine's smart-case default with the explicit UI option.
        let pattern = format!(
            "(?{}i:{pattern})",
            if query.case_sensitive { "-" } else { "" }
        );
        let Ok(mut regex) = RegexSearch::new(&pattern) else {
            return SearchResult {
                invalid: true,
                ..Default::default()
            };
        };
        let mut term = self.term.lock();
        let content_revision = self.content_revision.load(Ordering::Acquire);
        let start = Point::new(term.topmost_line(), Column(0));
        let end = Point::new(term.bottommost_line(), term.last_column());
        let mut matches: Vec<_> = RegexIter::new(start, end, Direction::Right, &*term, &mut regex)
            .take(MAX_MATCHES + 1)
            .collect();
        let truncated = matches.len() > MAX_MATCHES;
        matches.truncate(MAX_MATCHES);
        let current_index = current
            .filter(|(_, revision)| *revision == content_revision)
            .and_then(|(point, _)| matches.iter().position(|m| *m.start() == point));
        let selected = if matches.is_empty() {
            None
        } else if let Some(direction) = navigation {
            Some(match (current_index, direction) {
                (Some(index), n) if n < 0 => (index + matches.len() - 1) % matches.len(),
                (Some(index), _) => (index + 1) % matches.len(),
                (None, n) if n < 0 => matches.len() - 1,
                (None, _) => 0,
            })
        } else {
            current_index
        };
        if navigation.is_some()
            && let Some(index) = selected
        {
            let point = *matches[index].start();
            term.scroll_to_point(point);
            // Leave room for the floating search toolbar above the match.
            let viewport_row = point.line.0 + term.grid().display_offset() as i32;
            if viewport_row < 4 && term.screen_lines() > 5 {
                term.scroll_display(alacritty_terminal::grid::Scroll::Delta(4 - viewport_row));
            }
            self.revision.fetch_add(1, Ordering::Release);
            self.updates.notify();
        }
        SearchResult {
            revision: self.revision.load(Ordering::Acquire),
            content_revision,
            matches,
            current: selected,
            truncated,
            invalid: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(rows: usize, cols: usize, bytes: &[u8]) -> Arc<Session> {
        let session = Session::remote("search-test".into(), rows, cols, Arc::new(|_| Ok(())));
        session.remote_output(bytes);
        session
    }

    #[test]
    fn searches_scrollback_and_soft_wrapped_unicode_without_changing_scroll() {
        let session = screen(3, 8, "first\r\n你好abcdef\r\nlast\r\nend".as_bytes());
        let offset = session.term.lock().grid().display_offset();
        let result = session.search_output(
            &SearchQuery {
                text: "你好abcdef".into(),
                ..Default::default()
            },
            None,
            None,
        );
        assert_eq!(result.matches.len(), 1);
        assert!(result.matches[0].start().line < result.matches[0].end().line);
        assert_eq!(session.term.lock().grid().display_offset(), offset);
        assert_eq!(
            session
                .search_output(
                    &SearchQuery {
                        text: "first".into(),
                        ..Default::default()
                    },
                    None,
                    None
                )
                .matches
                .len(),
            1
        );
    }

    #[test]
    fn explicit_case_regex_and_literal_modes_are_independent() {
        let session = screen(5, 30, b"Foo foo f.o");
        let mut query = SearchQuery {
            text: "Foo".into(),
            ..Default::default()
        };
        assert_eq!(session.search_output(&query, None, None).matches.len(), 2);
        query.case_sensitive = true;
        assert_eq!(session.search_output(&query, None, None).matches.len(), 1);
        query.text = "f.o".into();
        assert_eq!(session.search_output(&query, None, None).matches.len(), 1);
        query.regex = true;
        query.case_sensitive = false;
        assert_eq!(session.search_output(&query, None, None).matches.len(), 3);
        query.text = "[".into();
        assert!(session.search_output(&query, None, None).invalid);
    }

    #[test]
    fn navigation_wraps_and_new_output_and_resize_are_researched() {
        let session = screen(3, 12, b"hit\r\nhit\r\nend\r\nend");
        let query = SearchQuery {
            text: "hit".into(),
            ..Default::default()
        };
        let first = session.search_output(&query, None, Some(1));
        assert_eq!(first.current, Some(0));
        assert!(session.term.lock().grid().display_offset() > 0);
        let last = session.search_output(
            &query,
            Some((*first.matches[0].start(), first.content_revision)),
            Some(-1),
        );
        assert_eq!(last.current, Some(1));
        session.remote_output(b"\r\nhit");
        assert!(
            session
                .search_output(
                    &query,
                    Some((*last.matches[1].start(), last.content_revision)),
                    None
                )
                .current
                .is_none()
        );
        session.remote_resize(4, 20);
        let offset = session.term.lock().grid().display_offset();
        assert_eq!(session.search_output(&query, None, None).matches.len(), 3);
        assert_eq!(session.term.lock().grid().display_offset(), offset);
    }
}
