use super::*;

/// Deterministic integration check using the real Windows text system and GPUI
/// draw path, an invisible window and an in-memory terminal (no shell/SSH).
pub(crate) fn run_render_check(output: std::path::PathBuf) -> bool {
    use std::sync::atomic::AtomicBool;
    let passed = Arc::new(AtomicBool::new(false));
    let result = passed.clone();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.spawn(async move |cx| {
                let report = render_check_inner(cx).await;
                let json = match report {
                    Ok(report) => {
                        result.store(true, Ordering::Release);
                        report
                    }
                    Err(error) => {
                        serde_json::json!({"passed": false, "error": format!("{error:#}")})
                    }
                };
                if std::fs::write(output, serde_json::to_vec_pretty(&json).unwrap()).is_err() {
                    result.store(false, Ordering::Release);
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    passed.load(Ordering::Acquire)
}

async fn render_check_inner(cx: &mut AsyncApp) -> anyhow::Result<serde_json::Value> {
    check_drag_release(cx).await?;
    use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
    use std::collections::VecDeque;
    let session = Session::remote("%render-check".into(), 60, 200, Arc::new(|_| Ok(())));
    for row in 0..60 {
        session.remote_output(
            format!(
                "\x1b[{};1H{}",
                row + 1,
                "The quick brown fox 0123456789 | ".repeat(6)
            )
            .as_bytes(),
        );
    }
    session.remote_output(
        "\x1b[2;1H中文 e\u{301} 🙂 中英混排 ⏸ ⏸\u{fe0e} ⏸\u{fe0f}\x1b[4;1H\x1b[1;3;4mStyled ASCII\x1b[0m".as_bytes(),
    );
    session.remote_output("\x1b[6;1H\x1b[2K┌──┬──┐ ╔══╦══╗ ╭──╮\x1b[7;1H\x1b[2K│  │  │ ║  ║  ║ │  │\x1b[8;1H\x1b[2K├──┼──┤ ╠══╬══╣ │  │\x1b[9;1H\x1b[2K└──┴──┘ ╚══╩══╝ ╰──╯\x1b[10;1H\x1b[2K██ ▄▄ ▀▀ ░▒▓".as_bytes());
    session.remote_output("\x1b[12;1H\x1b[2KBody  \x1b[2mSubtitle / tool output  \x1b[36mCode label  │\x1b[0m  \x1b[1;2m›\x1b[0m".as_bytes());
    session.remote_output("\x1b[14;1H\x1b[2K\x1b[4:1mSingle\x1b[0m  \x1b[4:2mDouble\x1b[0m  \x1b[4:3mCurly\x1b[0m  \x1b[4:4mDotted\x1b[0m  \x1b[4:5mDashed\x1b[0m\x1b[15;1H\x1b[2K\x1b[31;1mBold red\x1b[0m  \x1b[2m│┼╬╔█\x1b[0m  \x1b[4:2;58:2::255:80:80mUnderline colour\x1b[0m  \x1b[9mStrike\x1b[0m ╱╳╲".as_bytes());
    let initial = session.render_snapshot(VecDeque::new()).0;
    let source = session.clone();
    let handle = cx.open_window(
        WindowOptions {
            show: false,
            focus: false,
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(1700.), px(1260.)),
            ))),
            ..Default::default()
        },
        move |_, cx| {
            cx.new(|cx| {
                let mut view = TerminalView::from_session(
                    source,
                    14.,
                    Palette::new(gpui_kit::component::ThemeMode::Dark),
                    cx,
                );
                view.apply_snapshot(initial, cx);
                view
            })
        },
    )?;
    // Output must reach the view without a manual draw requesting the capture.
    handle.update(cx, |view, _, cx| view.set_visible(true, cx))?;
    session.remote_output(b"\x1b[1;1H!");
    let output_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if handle.update(cx, |view, _, _| {
            !view.snapshot_in_flight
                && view.snapshot.revision == session.revision.load(Ordering::Acquire)
                && view.snapshot.rows[0][0].c == '!'
        })? {
            break;
        }
        anyhow::ensure!(
            Instant::now() < output_deadline,
            "output snapshot waited for a draw"
        );
        cx.background_executor()
            .timer(Duration::from_millis(1))
            .await;
    }
    handle.update(cx, |view, _, cx| view.set_visible(false, cx))?;
    let hidden_revision = handle.update(cx, |view, _, _| view.snapshot.revision)?;
    session.remote_output(b"\x1b[1;1H?");
    handle.update(cx, |view, _, cx| {
        view.changed(cx);
        assert!(!view.snapshot_in_flight);
        assert_eq!(view.snapshot.revision, hidden_revision);
        view.set_visible(true, cx);
    })?;
    let resume_deadline = Instant::now() + Duration::from_secs(5);
    while handle.update(cx, |view, _, _| view.snapshot_in_flight)? {
        anyhow::ensure!(
            Instant::now() < resume_deadline,
            "visible snapshot did not resume"
        );
        cx.background_executor()
            .timer(Duration::from_millis(1))
            .await;
    }
    anyhow::ensure!(
        handle.update(cx, |view, _, _| view.snapshot.rows[0][0].c == '?')?,
        "hidden output was lost"
    );
    // The CPU benchmarks below explicitly capture each update. Stop the live
    // consumer so it cannot consume damage before the measured capture does.
    handle.update(cx, |view, _, _| view._output_task = Task::ready(()))?;
    if let Some(path) = std::env::var_os("TSHELL_LIGATURE_TEST_FONT") {
        let bytes = std::fs::read(path)?;
        cx.update_window(handle.into(), |_, window, _| {
            window
                .text_system()
                .add_fonts(vec![std::borrow::Cow::Owned(bytes)])
        })??;
        handle.update(cx, |view, window, _| {
            let before_family = view.font_family.clone();
            view.font_family = "JetBrains Mono".into();
            let probe = view.text_run("M", view.palette.text, view.palette.text, Flags::empty());
            view.cell_width = f32::from(
                window
                    .text_system()
                    .shape_line("M".into(), px(view.font_size), &[probe], None)
                    .width,
            );
            let cells: Arc<[Cell]> = "== != => -> <= >= === !=="
                .chars()
                .map(|c| Cell {
                    c,
                    ..Cell::default()
                })
                .collect::<Vec<_>>()
                .into();
            let off = view.shape_row(cells.clone(), window);
            view.ligatures = true;
            let on = view.shape_row(cells, window);
            let glyphs = |row: &PaintedRow| {
                row.text
                    .iter()
                    .flat_map(|t| {
                        t.shaped
                            .runs
                            .iter()
                            .flat_map(|r| r.glyphs.iter().map(|g| g.id))
                    })
                    .collect::<Vec<_>>()
            };
            anyhow::ensure!(
                glyphs(&off) != glyphs(&on),
                "enabling ligatures did not change glyphs"
            );
            anyhow::ensure!(
                on.text.iter().all(|t| ligature_grid_aligned(
                    &t.shaped,
                    t.columns,
                    view.cell_width
                )),
                "ligature clusters escaped the cell grid"
            );
            view.ligatures = false;
            view.font_family = before_family;
            Ok::<_, anyhow::Error>(())
        })??;
    }
    // On Windows, show:false defers the initial WindowPlacement entirely. A
    // hidden window can therefore report an invalid/zero viewport until an
    // explicit native resize has completed. Never benchmark clipped-away text.
    cx.update_window(handle.into(), |_, window, _| {
        window.resize(size(px(1700.), px(1260.)));
    })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let viewport = cx.update_window(handle.into(), |_, window, _| window.viewport_size())?;
        if viewport.width >= px(1600.) && viewport.height >= px(1260.) {
            break;
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "hidden viewport did not initialize: {viewport:?}"
        );
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
    // Draw through the window, outside any entity update/borrow of its root.
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })?;
    handle.update(cx, |view, window, cx| window.focus(&view.focus, cx))?;
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })?;
    anyhow::ensure!(
        handle.update(cx, |view, _, _| view.cursor_focused && view.cursor_visible)?,
        "focused cursor did not start visible"
    );
    let blink_deadline = Instant::now() + Duration::from_secs(2);
    while handle.update(cx, |view, _, _| view.cursor_visible)? {
        anyhow::ensure!(
            Instant::now() < blink_deadline,
            "cursor did not blink after 600 ms"
        );
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    cx.update_window(handle.into(), |_, window, cx| window.blur(cx))?;
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })?;
    anyhow::ensure!(
        handle.update(cx, |view, _, _| !view.cursor_focused
            && !view.cursor_visible)?,
        "blurred terminal cursor remained visible"
    );
    let baseline = handle.update(cx, |view, _, _| view.painted_rows.clone())?;
    anyhow::ensure!(baseline.len() == 60, "initial rows not rendered");
    anyhow::ensure!(
        baseline[13]
            .decorations
            .iter()
            .any(|d| d.kind == Some(crate::terminal_decorations::Underline::Double)),
        "extended underline not drawn"
    );
    let faint_ink = handle.update(cx, |view, _, _| {
        view.text_run("faint", 0x273244, 0x273244, Flags::DIM).color
    })?;
    anyhow::ensure!(
        (faint_ink.a - 0.5).abs() < 0.001,
        "DIM was flattened into opaque RGB"
    );
    anyhow::ensure!(
        baseline[6].text.is_empty() && !baseline[6].glyphs.is_empty(),
        "border-only row used font shaping"
    );
    handle.update(cx, |view, _, _| {
        anyhow::ensure!(view.row_views[6].is_some(), "border-only row was culled");
        Ok::<_, anyhow::Error>(())
    })??;
    let initial_geometry = handle.update(cx, |view, window, _| {
        (
            view.row_paints.get(),
            view.bounds,
            window.viewport_size(),
            view.cell_width,
            view.line_height,
        )
    })?;
    anyhow::ensure!(
        initial_geometry.0 >= 60,
        "initial text row paint callbacks did not run: {initial_geometry:?}"
    );
    let batches = baseline.iter().map(|row| row.text.len()).sum::<usize>();
    anyhow::ensure!(
        batches < 300,
        "ASCII grid alignment degraded to per-cell drawing: {batches}"
    );

    let mut draw_times = Vec::new();
    for step in 0..120 {
        let source = session.clone();
        let snap = cx
            .background_executor()
            .spawn(async move {
                source
                    .render_snapshot(VecDeque::from([
                        RenderCommand::Begin(
                            Anchor {
                                point: GridPoint::new(Line(0), Column(0)),
                                side: Side::Left,
                            },
                            SelectionType::Simple,
                        ),
                        RenderCommand::Extend(Anchor {
                            point: GridPoint::new(Line(step % 60), Column(step as usize % 200)),
                            side: Side::Right,
                        }),
                    ]))
                    .0
            })
            .await;
        anyhow::ensure!(snap.copied_rows == 0, "selection rebuilt terminal rows");
        handle.update(cx, |view, _, cx| {
            view.snapshot = snap;
            cx.notify();
        })?;
        let started = Instant::now();
        let paints_before = handle.update(cx, |view, _, _| view.row_paints.get())?;
        cx.update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })?;
        draw_times.push(started.elapsed().as_micros());
        let painted = handle.update(cx, |view, _, _| view.row_paints.get() - paints_before)?;
        anyhow::ensure!(
            painted == 0,
            "selection repainted {painted} cached text rows"
        );
        let reused = handle.update(cx, |view, _, _| {
            baseline
                .iter()
                .zip(&view.painted_rows)
                .all(|(a, b)| Arc::ptr_eq(a, b))
        })?;
        anyhow::ensure!(reused, "selection rebuilt shaped rows");
    }

    // Exercise mailbox coalescing without a next-frame delay; release must
    // commit the final half-cell anchor.
    handle.update(cx, |view, window, cx| {
        view.selecting = true;
        view.selection_point = None;
        for step in 0..1_000 {
            view.move_selection(
                &MouseMoveEvent {
                    position: view.bounds.origin
                        + point(
                            px((step % 100) as f32 * view.cell_width),
                            px(view.line_height * 2.),
                        ),
                    pressed_button: Some(MouseButton::Left),
                    ..Default::default()
                },
                window,
                cx,
            );
        }
        assert!(view.interactions_in_flight);
        assert!(!view.mailbox.is_empty());
        view.finish_selection(cx);
    })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.update(cx, |view, _, _| {
        view.snapshot_in_flight || view.interactions_in_flight
    })? {
        anyhow::ensure!(Instant::now() < deadline, "snapshot job did not finish");
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
    let selection_end = handle.update(cx, |view, _, _| view.snapshot.selection.map(|s| s.end))?;
    anyhow::ensure!(
        selection_end == Some(GridPoint::new(Line(2), Column(98))),
        "final pointer anchor was lost: {selection_end:?}"
    );

    let mut sparse_updates = Vec::new();
    for dense in [false, true] {
        for cached in [false, true] {
            sparse_updates.push(check_sparse_updates(handle, &session, dense, cached, cx).await?);
        }
    }

    for size in [10., 14., 24.] {
        let before = handle.update(cx, |view, _, _| view.row_paints.get())?;
        handle.update(cx, |view, _, cx| {
            view.font_size = size;
            view.palette = Palette::new(gpui_kit::component::ThemeMode::Light);
            cx.notify();
        })?;
        cx.update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })?;
        let correct = handle.update(cx, |view, _, _| {
            view.measured_font_size == size.to_bits()
                && view.painted_palette == view.palette
                && view.painted_rows.len() == 60
                && view.row_paints.get() > before
        })?;
        anyhow::ensure!(correct, "font/theme cache invalidation failed");
    }
    // Returning to the original metrics must rebuild, then settle to zero work.
    handle.update(cx, |view, _, cx| {
        view.font_size = 14.;
        view.palette = Palette::new(gpui_kit::component::ThemeMode::Dark);
        cx.notify();
    })?;
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })?;
    let before = handle.update(cx, |view, _, _| view.row_paints.get())?;
    handle.update(cx, |_, _, cx| cx.notify())?;
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })?;
    anyhow::ensure!(
        handle.update(cx, |view, _, _| view.row_paints.get())? == before,
        "unchanged rows did not settle after style invalidation"
    );
    // OSC palette updates change appearance even when every cell is unchanged.
    let source = session.clone();
    let recolored = cx
        .background_executor()
        .spawn(async move {
            source.remote_output(b"\x1b]10;#00ff00\x07");
            source.render_snapshot(VecDeque::new()).0
        })
        .await;
    let before = handle.update(cx, |view, _, cx| {
        view.snapshot = recolored;
        cx.notify();
        view.row_paints.get()
    })?;
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })?;
    anyhow::ensure!(
        handle.update(cx, |view, _, _| view.row_paints.get() - before)? == 60,
        "OSC foreground change did not invalidate all text rows"
    );
    // Removing rows must also remove their cached entities. Re-expansion must
    // paint the new viewport, rather than replay rows from before the resize.
    for (rows, cols) in [(40, 120), (60, 200)] {
        session.remote_resize(rows, cols);
        let resized = session.render_snapshot(VecDeque::new()).0;
        let before = handle.update(cx, |view, _, cx| {
            view.snapshot = resized;
            cx.notify();
            view.row_paints.get()
        })?;
        cx.update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })?;
        anyhow::ensure!(
            handle.update(cx, |view, _, _| view.row_views.len() == rows
                && view.row_paints.get() - before
                    == view.row_views.iter().flatten().count())?,
            "resized viewport did not replace its row cache"
        );
    }
    for factor in [1.2, 2., 1.] {
        handle.update(cx, |view, _, cx| {
            view.line_height_scale = factor.into();
            cx.notify();
        })?;
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })?;
        handle.update(cx, |view, window, _| {
            let text = window.text_system();
            let face = text.resolve_font(&font(view.font_family.clone()));
            let natural = f32::from(text.ascent(face, px(view.font_size)))
                + f32::from(text.descent(face, px(view.font_size))).abs();
            let dpr = window.scale_factor();
            let expected = ((natural * dpr).ceil() * factor).floor() / dpr;
            anyhow::ensure!(
                view.line_height == expected,
                "line height did not follow font metrics"
            );
            let anchor = view.grid_anchor(view.bounds.origin + point(px(1.), px(expected * 2.25)));
            anyhow::ensure!(
                anchor.point.line.0 + view.snapshot.display_offset as i32 == 2,
                "pointer row did not follow line height"
            );
            Ok::<_, anyhow::Error>(())
        })??;
    }
    draw_times.sort_unstable();
    Ok(serde_json::json!({
        "passed": true, "custom_tui_glyphs_checked": true, "ligature_font_checked": std::env::var_os("TSHELL_LIGATURE_TEST_FONT").is_some(), "grid": "200x60", "selection_frames": 120,
        "viewport_pixels": [f32::from(initial_geometry.2.width), f32::from(initial_geometry.2.height)],
        "initial_painted_text_rows": initial_geometry.0,
        "pointer_events_coalesced": 1000, "selection_shaped_rows": 0,
        "selection_copied_rows": 0, "text_batches": batches,
        "selection_painted_text_rows": 0,
        "draw_cpu_median_us": draw_times[60], "draw_cpu_p95_us": draw_times[114],
        "draw_cpu_p99_us": draw_times[118],
        "font_sizes_checked": [10,14,24], "theme_invalidation": true,
        "osc_color_invalidation": true, "resize_invalidation": true,
        "drag_release_preserves_copy": true,
        "font_line_height_and_pointer_checked": true,
        "sparse_updates": sparse_updates,
        "includes_gpu_presentation": false,
    }))
}

async fn check_drag_release(cx: &mut AsyncApp) -> anyhow::Result<()> {
    use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
    use std::collections::VecDeque;

    for lines in [3, -3] {
        for block in [false, true] {
            for stale_snapshot in [false, true] {
                let session = Session::remote("%drag-release".into(), 5, 20, Arc::new(|_| Ok(())));
                for row in 0..30 {
                    session.remote_output(format!("row {row:02} abcdefghijk\r\n").as_bytes());
                }
                let before = session
                    .render_snapshot(VecDeque::from([
                        RenderCommand::Scroll(Scroll::Delta(10)),
                        RenderCommand::Begin(
                            Anchor {
                                point: GridPoint::new(Line(-8), Column(3)),
                                side: Side::Left,
                            },
                            if block {
                                SelectionType::Block
                            } else {
                                SelectionType::Simple
                            },
                        ),
                    ]))
                    .0;
                let position = point(px(80.), px(if lines > 0 { -10. } else { 110. }));
                let old_anchor = hit_test(
                    80.,
                    f32::from(position.y),
                    10.,
                    20.,
                    5,
                    20,
                    before.display_offset,
                );
                let (after, expected) = session.render_snapshot(VecDeque::from([
                    RenderCommand::DragScroll {
                        lines,
                        column: 8,
                        side: Side::Left,
                        block,
                    },
                    RenderCommand::Copy,
                ]));
                anyhow::ensure!(!expected.is_empty(), "drag must select text");
                let source = session.clone();
                let view = cx.new(|cx| {
                    let mut view = TerminalView::from_session(
                        source,
                        14.,
                        Palette::new(gpui_kit::component::ThemeMode::Dark),
                        cx,
                    );
                    view._output_task = Task::ready(());
                    view.snapshot = if stale_snapshot { before } else { after };
                    view.bounds = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(100.)));
                    view.cell_width = 10.;
                    view.line_height = 20.;
                    view.selecting = true;
                    view.block_selection = block;
                    view.selection_point = Some(old_anchor);
                    view.drag_position = Some(position);
                    view.drag_scroll_task = Some(Task::ready(()));
                    view.mouse_up(
                        &MouseUpEvent {
                            button: MouseButton::Left,
                            position,
                            ..Default::default()
                        },
                        cx,
                    );
                    assert!(
                        !view.selecting
                            && view.drag_position.is_none()
                            && view.drag_scroll_task.is_none()
                    );
                    view
                });
                let deadline = Instant::now() + Duration::from_secs(5);
                while view.read_with(cx, |view, _| view.interactions_in_flight) {
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "release interaction did not finish"
                    );
                    cx.background_executor()
                        .timer(Duration::from_millis(1))
                        .await;
                }
                let copied = session.apply_interactions(VecDeque::from([RenderCommand::Copy]));
                anyhow::ensure!(
                    copied == expected,
                    "release changed copied text: lines={lines}, block={block}, stale={stale_snapshot}; expected={expected:?}, got={copied:?}"
                );
            }
        }
    }
    Ok(())
}

async fn check_sparse_updates(
    handle: WindowHandle<TerminalView>,
    session: &Arc<Session>,
    dense: bool,
    cached: bool,
    cx: &mut AsyncApp,
) -> anyhow::Result<serde_json::Value> {
    use std::collections::VecDeque;
    session.remote_output(b"\x1b[0m\x1b[2J\x1b[H");
    if dense {
        for row in 0..60 {
            session.remote_output(
                format!("\x1b[{};1H{}", row + 1, "full screen 0123456789 ".repeat(9)).as_bytes(),
            );
        }
    }
    let snapshot = session
        .render_snapshot(VecDeque::from([RenderCommand::Input(Vec::new())]))
        .0;
    handle.update(cx, |view, _, cx| {
        view.cache_row_paint = cached;
        view.snapshot = snapshot;
        cx.notify();
    })?;
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })?;
    let mut times = Vec::new();
    let mut total_painted = 0;
    for step in 0..120 {
        let source = session.clone();
        let snapshot = cx
            .background_executor()
            .spawn(async move {
                source.remote_output(if step % 2 == 0 {
                    b"\x1b[30;20Hj"
                } else {
                    b"\x1b[30;20Hk"
                });
                source.render_snapshot(VecDeque::new()).0
            })
            .await;
        anyhow::ensure!(
            snapshot.copied_rows == 1,
            "sparse update copied {} rows",
            snapshot.copied_rows
        );
        handle.update(cx, |view, _, cx| {
            view.snapshot = snapshot;
            cx.notify();
        })?;
        let before = handle.update(cx, |view, _, _| view.row_paints.get())?;
        let start = Instant::now();
        cx.update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })?;
        times.push(start.elapsed().as_micros());
        let painted = handle.update(cx, |view, _, _| view.row_paints.get() - before)?;
        anyhow::ensure!(
            painted == if cached || !dense { 1 } else { 60 },
            "sparse update painted {painted} rows (cached={cached})"
        );
        total_painted += painted;
    }
    times.sort_unstable();
    Ok(serde_json::json!({
        "content": if dense { "full" } else { "mostly_empty" },
        "row_scene_cache": cached, "frames": 120,
        "painted_text_rows_per_frame": total_painted / 120,
        "draw_cpu_median_us": times[60], "draw_cpu_p95_us": times[114], "draw_cpu_p99_us": times[118],
    }))
}
