use super::*;

// xterm.js 6.0.0 SelectionService: 50 ms, 50 px threshold, at most 15 rows.
fn scroll_lines(y: f32, height: f32) -> i32 {
    let distance = if y < 0. {
        y
    } else if y > height {
        y - height
    } else {
        return 0;
    };
    let fraction = distance.clamp(-50., 50.) / 50.;
    // JavaScript Math.round rounds negative ties towards positive infinity.
    -(fraction.signum() as i32 + (fraction * 14. + 0.5).floor() as i32)
}

impl TerminalView {
    pub(super) fn update_drag_scroll(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.drag_position = Some(position);
        if scroll_lines(
            f32::from(position.y - self.bounds.top()),
            self.snapshot.rows.len() as f32 * self.line_height,
        ) == 0
        {
            self.drag_scroll_task = None;
            return;
        }
        if self.drag_scroll_task.is_some() {
            return;
        }
        self.drag_scroll_task = Some(cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let keep = view
                    .update(cx, |this, cx| {
                        if !this.selecting || !this.visible {
                            return false;
                        }
                        let Some(position) = this.drag_position else {
                            return false;
                        };
                        let lines = scroll_lines(
                            f32::from(position.y - this.bounds.top()),
                            this.snapshot.rows.len() as f32 * this.line_height,
                        );
                        if lines == 0 {
                            return false;
                        }
                        // Backpressure: never accumulate timer work behind terminal IO.
                        if !this.interactions_in_flight {
                            let anchor = this.grid_anchor(position);
                            this.queue_render(
                                RenderCommand::DragScroll {
                                    lines,
                                    column: anchor.point.column.0,
                                    side: anchor.side,
                                    block: this.block_selection,
                                },
                                cx,
                            );
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::scroll_lines;
    #[test]
    fn xterm_drag_scroll_reference() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/xterm-selection-scroll.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            assert_eq!(
                scroll_lines(case["y"].as_f64().unwrap() as f32, 100.),
                -(case["amount"].as_i64().unwrap() as i32),
                "{case}"
            );
        }
    }
}
