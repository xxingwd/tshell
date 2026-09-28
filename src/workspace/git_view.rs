use super::git::{DiffLine, GitChange, LineKind};
use super::*;

pub(super) struct DiffView {
    pub(super) change: GitChange,
    pub(super) content: Result<DiffContent, String>,
    pub(super) scroll: UniformListScrollHandle,
    pub(super) horizontal: [ScrollHandle; 3],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DiffMode {
    Inline,
    SideBySide,
}

struct CodeLine {
    number: usize,
    text: String,
    changed: bool,
}
enum DiffRow {
    Code {
        old: Option<CodeLine>,
        new: Option<CodeLine>,
    },
    Gap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum VisibleRow {
    Line(usize),
    Fold(std::ops::Range<usize>),
}

// Merge overlapping context windows, then represent each omitted run once.
fn compact_rows(changed: impl IntoIterator<Item = bool>) -> Vec<VisibleRow> {
    let changed = changed.into_iter().collect::<Vec<_>>();
    let mut keep = vec![false; changed.len()];
    for (index, changed) in changed.iter().enumerate() {
        if *changed {
            let end = (index + 4).min(keep.len());
            keep[index.saturating_sub(3)..end].fill(true);
        }
    }
    let mut rows = Vec::new();
    let mut index = 0;
    while index < keep.len() {
        if keep[index] {
            rows.push(VisibleRow::Line(index));
            index += 1;
        } else {
            let start = index;
            while index < keep.len() && !keep[index] {
                index += 1;
            }
            rows.push(VisibleRow::Fold(start..index));
        }
    }
    rows
}

pub(super) struct DiffContent {
    rows: Vec<DiffRow>,
    inline: Vec<DiffLine>,
    columns: [usize; 2],
    compact: [Vec<VisibleRow>; 2],
    notice: String,
}
impl DiffContent {
    pub(super) fn new(lines: Vec<DiffLine>) -> Self {
        let binary = lines
            .iter()
            .any(|line| line.kind == LineKind::Header && line.text.starts_with("Binary files "));
        let mut inline = Vec::new();
        for line in &lines {
            if line.kind == LineKind::Header {
                if line.text.starts_with("@@ ") && !inline.is_empty() {
                    inline.push(line.clone());
                }
            } else {
                let mut line = line.clone();
                line.text = line.text.get(1..).unwrap_or_default().replace('\t', "    ");
                inline.push(line);
            }
        }
        let mut rows = Vec::new();
        let mut removed = Vec::new();
        let mut added = Vec::new();
        for line in lines {
            match line.kind {
                LineKind::Removed => removed.push(CodeLine::new(line.old, line.text, true)),
                LineKind::Added => added.push(CodeLine::new(line.new, line.text, true)),
                LineKind::Context => {
                    Self::flush(&mut rows, &mut removed, &mut added);
                    rows.push(DiffRow::Code {
                        old: Some(CodeLine::new(line.old, line.text.clone(), false)),
                        new: Some(CodeLine::new(line.new, line.text, false)),
                    });
                }
                LineKind::Header if line.text.starts_with("@@ ") => {
                    Self::flush(&mut rows, &mut removed, &mut added);
                    if !rows.is_empty() {
                        rows.push(DiffRow::Gap);
                    }
                }
                _ => {}
            }
        }
        Self::flush(&mut rows, &mut removed, &mut added);
        let mut columns = [0, 0];
        for row in &rows {
            if let DiffRow::Code { old, new } = row {
                for (index, line) in [old, new].into_iter().enumerate() {
                    if let Some(line) = line {
                        columns[index] = columns[index].max(
                            line.text
                                .chars()
                                .map(|c| if c.is_ascii() { 1 } else { 2 })
                                .sum(),
                        );
                    }
                }
            }
        }
        let compact = [
            compact_rows(rows.iter().map(|row| match row {
                DiffRow::Gap => true,
                DiffRow::Code { old, new } => {
                    old.as_ref().is_some_and(|line| line.changed)
                        || new.as_ref().is_some_and(|line| line.changed)
                }
            })),
            compact_rows(inline.iter().map(|line| line.kind != LineKind::Context)),
        ];
        Self {
            rows,
            inline,
            compact,
            columns,
            notice: if binary {
                crate::t!("git.binary_notice").to_string()
            } else {
                crate::t!("git.no_text_change").to_string()
            },
        }
    }

    fn flush(rows: &mut Vec<DiffRow>, removed: &mut Vec<CodeLine>, added: &mut Vec<CodeLine>) {
        let count = removed.len().max(added.len());
        let mut old = removed.drain(..);
        let mut new = added.drain(..);
        rows.extend((0..count).map(|_| DiffRow::Code {
            old: old.next(),
            new: new.next(),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::{DiffContent, DiffRow, VisibleRow, compact_rows};
    use crate::workspace::git;

    #[test]
    fn compact_context_merges_nearby_changes_and_keeps_three_lines() {
        let rows = compact_rows((0..30).map(|index| index == 8 || index == 14));
        let mut expected = vec![VisibleRow::Fold(0..5)];
        expected.extend((5..18).map(VisibleRow::Line));
        expected.push(VisibleRow::Fold(18..30));
        assert_eq!(rows, expected);
        assert_eq!(
            compact_rows([true, false, false]),
            vec![
                VisibleRow::Line(0),
                VisibleRow::Line(1),
                VisibleRow::Line(2)
            ]
        );
        assert!(compact_rows([]).is_empty());
    }

    #[test]
    fn comparison_hides_patch_metadata_and_aligns_unequal_changes() {
        let content = DiffContent::new(git::parse_patch(
            "diff --git a/f b/f\nindex 123..456 100644\n--- a/f\n+++ b/f\n@@ -2,2 +2,3 @@\n---old\n+++new\n+extra\n context\n@@ -20 +21 @@\n-tail\n+end\n",
        ));
        assert_eq!(content.rows.len(), 5);
        let DiffRow::Code {
            old: Some(old),
            new: Some(new),
        } = &content.rows[0]
        else {
            panic!("replacement must align");
        };
        assert_eq!((&*old.text, &*new.text), ("--old", "++new"));
        assert_eq!((old.number, new.number), (2, 2));
        assert!(old.changed && new.changed);
        assert!(matches!(
            &content.rows[1],
            DiffRow::Code {
                old: None,
                new: Some(_)
            }
        ));
        let DiffRow::Code {
            old: Some(old),
            new: Some(new),
        } = &content.rows[2]
        else {
            panic!("context must align");
        };
        assert_eq!((old.number, new.number), (3, 4));
        assert!(!old.changed && !new.changed);
        assert!(matches!(content.rows[3], DiffRow::Gap));
    }

    #[test]
    fn binary_and_rename_only_have_concise_empty_states() {
        let binary = DiffContent::new(git::parse_patch(
            "diff --git a/f b/f\nBinary files a/f and b/f differ\n",
        ));
        assert!(binary.rows.is_empty());
        assert!(binary.notice.contains("Binary") || binary.notice.contains("二进制"));
        let rename = DiffContent::new(git::parse_patch(
            "diff --git a/f b/g\nsimilarity index 100%\nrename from f\nrename to g\n",
        ));
        assert!(rename.rows.is_empty());
        assert!(rename.notice.contains("rename") || rename.notice.contains("重命名"));
    }

    #[test]
    fn inline_keeps_patch_order_and_both_line_numbers() {
        let content = DiffContent::new(git::parse_patch(
            "--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n-old one\n-old two\n+new one\n+new two\n same\n@@ -9 +9 @@\n-last\n+tail\n",
        ));
        assert_eq!(content.inline.len(), 8);
        assert_eq!(
            content
                .inline
                .iter()
                .take(5)
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["old one", "old two", "new one", "new two", "same"]
        );
        assert_eq!(
            (content.inline[0].old, content.inline[0].new),
            (Some(1), None)
        );
        assert_eq!(
            (content.inline[2].old, content.inline[2].new),
            (None, Some(1))
        );
        assert_eq!(
            (content.inline[4].old, content.inline[4].new),
            (Some(3), Some(3))
        );
        assert_eq!(content.inline[5].kind, git::LineKind::Header);
    }
}
impl CodeLine {
    fn new(number: Option<usize>, text: String, changed: bool) -> Self {
        Self {
            number: number.unwrap_or(0),
            text: text.get(1..).unwrap_or_default().replace('\t', "    "),
            changed,
        }
    }
}

impl AppView {
    fn open_git_diff(&mut self, change: GitChange, window: &mut Window, cx: &mut Context<Self>) {
        self.git_diff_request += 1;
        let request = self.git_diff_request;
        let host = self.hosts[self.active].config.clone();
        self.git_diff = Some(DiffView {
            change: change.clone(),
            scroll: UniformListScrollHandle::new(),
            horizontal: std::array::from_fn(|_| ScrollHandle::new()),
            content: Err(crate::t!("git.reading_diff").to_string()),
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    git::patch(&change, host.as_ref())
                        .map(DiffContent::new)
                        .map_err(|error| format!("{error:#}"))
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                if this.git_diff_request != request {
                    return;
                }
                if let Some(diff) = &mut this.git_diff {
                    diff.content = result;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn git_row(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let change = &self.git_changes[index];
        let selected = self
            .git_diff
            .as_ref()
            .is_some_and(|diff| diff.change.path == change.path);
        let relative_path = change
            .path
            .strip_prefix(&change.root)
            .unwrap_or(&change.path);
        let name = relative_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let directory = relative_path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let status = change.label();
        let color = match status {
            "D" => p.ansi[1],
            "A" => p.ansi[2],
            _ => p.accent,
        };
        let change = change.clone();
        ListItem::new(SharedString::from(format!("git-entry-{index}")))
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .h(px(26.))
            .min_h(px(26.))
            .px_1()
            .py_0()
            .rounded_sm()
            .text_size(px(12.))
            .line_height(relative(1.35))
            .when(selected, |row| row.bg(rgb(p.selected)))
            .child(
                div()
                    .w_full()
                    .h(px(26.))
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .overflow_hidden()
                    .child(
                        div()
                            .w(px(14.))
                            .flex_shrink_0()
                            .text_center()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(color))
                            .child(status),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .overflow_hidden()
                            .child(div().truncate().flex_shrink_0().max_w_full().child(name))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(10.))
                                    .text_color(rgb(p.muted))
                                    .child(directory),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .justify_end()
                            .gap(px(5.))
                            .text_size(px(11.))
                            .children(change.stats.map(|stats| {
                                div()
                                    .flex()
                                    .gap(px(5.))
                                    .child(
                                        div()
                                            .text_color(rgb(p.ansi[1]))
                                            .child(format!("−{}", stats.removed)),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(p.ansi[2]))
                                            .child(format!("+{}", stats.added)),
                                    )
                            }))
                            .when(change.stats.is_none(), |row| {
                                row.child(div().text_color(rgb(p.muted)).child("—"))
                            }),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_git_diff(change.clone(), window, cx)
            }))
    }

    pub(super) fn git_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let changes = uniform_list(
            ("git-files", self.git_request),
            self.git_changes.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| this.git_row(index, cx).into_any_element())
                    .collect()
            }),
        )
        .size_full();
        div()
            .size_full()
            .flex()
            .flex_col()
            .pt_1()
            .when_some(self.git_error.clone(), |panel, error| {
                panel.child(div().px_3().py_2().text_color(rgb(p.muted)).child(error))
            })
            .when(
                self.git_changes.is_empty() && self.git_error.is_none(),
                |panel| {
                    panel.child(
                        div()
                            .px_3()
                            .py_2()
                            .text_size(px(12.))
                            .text_color(rgb(p.muted))
                            .child(crate::t!("git.clean")),
                    )
                },
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .px_1()
                    .child(changes),
            )
            .into_any_element()
    }

    pub(super) fn set_diff_mode(&mut self, mode: DiffMode, cx: &mut Context<Self>) {
        if self.git_diff_mode == mode {
            return;
        }
        self.git_diff_mode = mode;
        if let Some(diff) = &mut self.git_diff {
            diff.scroll = UniformListScrollHandle::new();
            diff.horizontal = std::array::from_fn(|_| ScrollHandle::new());
        }
        cx.notify();
    }

    fn diff_pane(&self, side: Option<usize>, cx: &mut Context<Self>) -> AnyElement {
        let Some(DiffView {
            content: Ok(content),
            scroll,
            horizontal,
            ..
        }) = &self.git_diff
        else {
            return div().into_any_element();
        };
        let p = self.terminal_palette;
        let pane = side.unwrap_or(2);
        let columns = side
            .map(|side| content.columns[side])
            .unwrap_or(content.columns[0].max(content.columns[1]));
        let width = columns as f32 * self.font_size.clamp(12., 17.) * 0.65
            + if side.is_some() { 84. } else { 133. };
        let projection = usize::from(side.is_none());
        let count = if self.git_diff_compact {
            content.compact[projection].len()
        } else if side.is_some() {
            content.rows.len()
        } else {
            content.inline.len()
        };
        let mut list = uniform_list(
            SharedString::from(format!("diff-{}-{pane}", self.git_diff_request)),
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let Some(DiffView {
                    content: Ok(content),
                    ..
                }) = &this.git_diff
                else {
                    return Vec::new();
                };
                range
                    .map(|index| {
                        let p = this.terminal_palette;
                        let index = if this.git_diff_compact {
                            match &content.compact[projection][index] {
                                VisibleRow::Line(source) => *source,
                                VisibleRow::Fold(_) => {
                                    return div()
                                        .id(("diff-fold", index))
                                        .w_full()
                                        .h(px(24.))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .bg(rgba((p.muted << 8) | 18))
                                        .text_color(rgb(p.muted))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgba((p.muted << 8) | 35)))
                                        .child("···")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(DiffView {
                                                content: Ok(content),
                                                ..
                                            }) = &mut this.git_diff
                                            {
                                                let rows = &mut content.compact[projection];
                                                if let Some(VisibleRow::Fold(range)) =
                                                    rows.get(index).cloned()
                                                {
                                                    rows.splice(
                                                        index..index + 1,
                                                        range.map(VisibleRow::Line),
                                                    );
                                                }
                                            }
                                            cx.notify();
                                        }))
                                        .into_any_element();
                                }
                            }
                        } else {
                            index
                        };
                        let (kind, numbers, text, missing) = if let Some(side) = side {
                            match &content.rows[index] {
                                DiffRow::Gap => (LineKind::Header, [None, None], "", false),
                                DiffRow::Code { old, new } => {
                                    let line = if side == 0 { old } else { new };
                                    let kind = if line.as_ref().is_some_and(|line| line.changed) {
                                        if side == 0 {
                                            LineKind::Removed
                                        } else {
                                            LineKind::Added
                                        }
                                    } else {
                                        LineKind::Context
                                    };
                                    (
                                        kind,
                                        [line.as_ref().map(|line| line.number), None],
                                        line.as_ref().map(|line| line.text.as_str()).unwrap_or(""),
                                        line.is_none(),
                                    )
                                }
                            }
                        } else {
                            let line = &content.inline[index];
                            (line.kind, [line.old, line.new], line.text.as_str(), false)
                        };
                        if kind == LineKind::Header {
                            return div()
                                .w_full()
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .px_3()
                                .bg(rgba((p.muted << 8) | 18))
                                .text_color(rgb(p.muted))
                                .text_size(px(11.))
                                .child("···")
                                .into_any_element();
                        }
                        let color = match kind {
                            LineKind::Removed => Some(p.ansi[1]),
                            LineKind::Added => Some(p.ansi[2]),
                            _ => None,
                        };
                        let background =
                            color
                                .map(|color| rgba((color << 8) | 24))
                                .unwrap_or_else(|| {
                                    if missing {
                                        rgba((p.muted << 8) | 14)
                                    } else {
                                        rgba(0)
                                    }
                                });
                        div()
                            .w_full()
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .bg(background)
                            .child(
                                div()
                                    .w(px(3.))
                                    .h_full()
                                    .flex_shrink_0()
                                    .when_some(color, |bar, color| {
                                        bar.bg(rgba((color << 8) | 150))
                                    }),
                            )
                            .children(
                                numbers
                                    .into_iter()
                                    .take(if side.is_some() { 1 } else { 2 })
                                    .map(|number| {
                                        div()
                                            .w(px(49.))
                                            .flex_shrink_0()
                                            .text_right()
                                            .pr_2()
                                            .text_color(rgb(color.unwrap_or(p.muted)))
                                            .child(
                                                number
                                                    .map(|number| number.to_string())
                                                    .unwrap_or_default(),
                                            )
                                    }),
                            )
                            .child(
                                div()
                                    .pl_2()
                                    .whitespace_nowrap()
                                    .text_color(rgb(p.text))
                                    .child(text.to_owned()),
                            )
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .track_scroll(scroll)
        .size_full()
        .min_w(px(width));
        list.style().restrict_scroll_to_axis = Some(true);
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(side == Some(0), |pane| {
                pane.border_r_1().border_color(rgb(self.palette.border))
            })
            .child(
                div()
                    .h(px(28.))
                    .w_full()
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(self.palette.border))
                    .text_size(px(11.))
                    .text_color(rgb(p.muted))
                    .child(match side {
                        Some(0) => "HEAD".to_string(),
                        Some(_) => crate::t!("git.worktree").to_string(),
                        None => crate::t!("git.head_to_worktree").to_string(),
                    }),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "diff-scroll-{}-{pane}",
                        self.git_diff_request
                    )))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_x_scroll()
                    .overflow_y_hidden()
                    .restrict_scroll_to_axis()
                    .track_scroll(&horizontal[pane])
                    .font_family(self.font_family.clone())
                    .text_size(px(self.font_size.clamp(12., 17.)))
                    .line_height(px(24.))
                    .child(list),
            )
            .into_any_element()
    }

    pub(super) fn git_surface(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let Some(diff) = &self.git_diff else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(rgb(p.muted))
                .child(crate::t!("git.select_file"))
                .into_any_element();
        };
        let path = diff.change.path.clone();
        let title = path
            .strip_prefix(&diff.change.root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let content = match &diff.content {
            Err(message) => div()
                .p_3()
                .text_size(px(12.))
                .text_color(rgb(p.muted))
                .child(message.clone())
                .into_any_element(),
            Ok(content) if content.rows.is_empty() => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(rgb(p.muted))
                .child(content.notice.clone())
                .into_any_element(),
            Ok(_) if self.git_diff_mode == DiffMode::Inline => self.diff_pane(None, cx),
            Ok(_) => div()
                .size_full()
                .flex()
                .child(self.diff_pane(Some(0), cx))
                .child(self.diff_pane(Some(1), cx))
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(32.))
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .text_size(px(12.))
                    .child(
                        Icon::new(IconName::FileCode)
                            .xsmall()
                            .text_color(rgb(p.muted)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(
                        Button::new("diff-toggle-mode")
                            .ghost()
                            .small()
                            .label(if self.git_diff_mode == DiffMode::Inline {
                                crate::t!("git.side_by_side")
                            } else {
                                crate::t!("git.inline")
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                let next = if this.git_diff_mode == DiffMode::Inline {
                                    DiffMode::SideBySide
                                } else {
                                    DiffMode::Inline
                                };
                                this.set_diff_mode(next, cx);
                            })),
                    )
                    .child(
                        Button::new("diff-toggle-context")
                            .ghost()
                            .small()
                            .label(if self.git_diff_compact {
                                crate::t!("git.full_file")
                            } else {
                                crate::t!("git.only_changes")
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git_diff_compact = !this.git_diff_compact;
                                if let Some(diff) = &mut this.git_diff {
                                    diff.scroll = UniformListScrollHandle::new();
                                }
                                cx.notify();
                            })),
                    )
                    .when(diff.change.label() != "D", |bar| {
                        bar.child(
                            Button::new("diff-open-file")
                                .ghost()
                                .small()
                                .disabled(self.file_loading.is_some())
                                .label(if self.file_loading.is_some() {
                                    crate::t!("git.opening")
                                } else {
                                    crate::t!("git.open_file")
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_path(path.clone(), window, cx)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(rgb(self.terminal_palette.terminal))
                    .child(content),
            )
            .into_any_element()
    }
}
