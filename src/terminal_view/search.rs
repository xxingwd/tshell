use super::*;
use crate::terminal::search::{SearchQuery, SearchResult};
use alacritty_terminal::selection::SelectionRange;
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Selectable, Sizable,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
    },
};

pub(super) struct SearchUi {
    input: Entity<InputState>,
    query: SearchQuery,
    result: SearchResult,
    generation: u64,
    pending: bool,
    busy: bool,
    navigation: Option<i8>,
    _subscription: Subscription,
}

#[cfg(debug_assertions)]
pub(crate) async fn check_search_links(cx: &mut AsyncApp) -> anyhow::Result<()> {
    use alacritty_terminal::grid::Dimensions;
    use gpui_kit::component::{Root, Theme, ThemeMode};
    use parking_lot::Mutex;
    let writes = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let captured = writes.clone();
    let session = Session::remote(
        "feature-check".into(),
        12,
        60,
        Arc::new(move |bytes| {
            captured.lock().push(bytes);
            Ok(())
        }),
    );
    for index in 0..40 {
        session.remote_output(format!("hit-{index} 中文\r\n").as_bytes());
    }
    let source = session.clone();
    let handle = cx.open_window(
        WindowOptions {
            show: false,
            focus: false,
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(620.), px(320.)),
            ))),
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| {
                TerminalView::from_session(source, 14., Palette::new(ThemeMode::Dark), cx)
            });
            cx.new(|cx| Root::new(view, window, cx).w(px(620.)).h(px(320.)))
        },
    )?;
    let view = handle.update(cx, |root, _, _| {
        root.view().clone().downcast::<TerminalView>().unwrap()
    })?;
    let draw = |cx: &mut AsyncApp| -> anyhow::Result<()> {
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })?;
        Ok(())
    };
    let send = |input, cx: &mut AsyncApp| -> anyhow::Result<()> {
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_event(input, cx)
        })?;
        Ok(())
    };
    view.update(cx, |view, cx| view.set_visible(true, cx));
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            let dimensions = session.term.lock().screen_lines();
            view.open_search(window, cx);
            anyhow::ensure!(
                session.term.lock().screen_lines() == dimensions,
                "search resized the terminal"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        for key in ["h", "i", "t"] {
            window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
        }
    })?;
    wait_search(&view, 40, cx).await?;
    anyhow::ensure!(
        writes.lock().is_empty(),
        "search typing leaked into the terminal"
    );
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        let search = view.read(cx).search.as_ref().unwrap();
        anyhow::ensure!(
            search.input.read(cx).focus_handle(cx).is_focused(window),
            "search input is not focused"
        );
        Ok::<_, anyhow::Error>(())
    })??;
    send(
        PlatformInput::KeyDown(KeyDownEvent {
            keystroke: Keystroke::parse("enter").unwrap(),
            is_held: false,
            prefer_character_input: false,
        }),
        cx,
    )?;
    wait_search(&view, 40, cx).await?;
    view.read_with(cx, |view, _| {
        anyhow::ensure!(
            view.search.as_ref().unwrap().result.current == Some(0),
            "Enter did not navigate search"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    let offset = session.term.lock().grid().display_offset();
    session.remote_output(b"hit-new\r\n");
    wait_search(&view, 41, cx).await?;
    anyhow::ensure!(
        session.term.lock().grid().display_offset() == offset + 1,
        "new output jumped away from search history"
    );
    for (name, mode, viewport) in [
        (
            "terminal-search-dark",
            ThemeMode::Dark,
            size(px(620.), px(320.)),
        ),
        (
            "terminal-search-light-narrow",
            ThemeMode::Light,
            size(px(220.), px(240.)),
        ),
    ] {
        handle.update(cx, |root, window, cx| {
            root.style().size.width = Some(viewport.width.into());
            root.style().size.height = Some(viewport.height.into());
            window.resize(viewport);
            Theme::change(mode, Some(window), cx);
            view.update(cx, |view, cx| {
                view.palette = Palette::new(mode);
                cx.notify();
            });
        })?;
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
        draw(cx)?;
        wait_search(&view, 41, cx).await?;
        draw(cx)?;
        view.read_with(cx, |view, _| {
            let result = &view.search.as_ref().unwrap().result;
            anyhow::ensure!(result.revision == view.snapshot.revision, "search highlight has stale revision: search={}, snapshot={}, offset={}, first={:?}", result.revision, view.snapshot.revision, view.snapshot.display_offset, result.matches.first());
            Ok::<_, anyhow::Error>(())
        })?;
        #[cfg(feature = "ui-check-screenshots")]
        if let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") {
            cx.update_window(handle.into(), |_, window, _| {
                let image = window.render_to_image()?;
                image.save(std::path::PathBuf::from(directory).join(format!("{name}.png")))?;
                Ok::<_, anyhow::Error>(())
            })??;
            cx.update_window(handle.into(), |_, window, cx| {
                let view = view.read(cx);
                let x = (view.cell_width * 0.5 * window.scale_factor()) as u32;
                let y = (view.line_height * 5.9 * window.scale_factor()) as u32;
                let image = window.render_to_image()?;
                let pixel = image.get_pixel(x, y).0;
                let color = ((pixel[0] as u32) << 16) | ((pixel[1] as u32) << 8) | pixel[2] as u32;
                anyhow::ensure!(color == view.palette.selection, "search match background absent: color={color:x}, expected={:x}, first={:?}, offset={}, x={x}, y={y}", view.palette.selection, view.search.as_ref().unwrap().result.matches.first(), view.snapshot.display_offset);
                Ok::<_, anyhow::Error>(())
            })??;
        }
        #[cfg(not(feature = "ui-check-screenshots"))]
        let _ = name;
    }
    send(
        PlatformInput::KeyDown(KeyDownEvent {
            keystroke: Keystroke::parse("escape").unwrap(),
            is_held: false,
            prefer_character_input: false,
        }),
        cx,
    )?;
    cx.update_window(handle.into(), |_, window, cx| {
        anyhow::ensure!(
            view.read(cx).search.is_none() && view.read(cx).focus.is_focused(window),
            "Escape did not close search and restore terminal focus"
        );
        Ok::<_, anyhow::Error>(())
    })??;
    session.remote_output(b"\x1bc\x1b[?1000h\x1b[?1006hhttps://example.org");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !view.read_with(cx, |view, _| {
        view.snapshot.revision == session.revision.load(Ordering::Acquire)
    }) {
        anyhow::ensure!(Instant::now() < deadline, "link snapshot did not arrive");
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
    draw(cx)?;
    let received = Arc::new(Mutex::new(None));
    let result = received.clone();
    let _subscription = cx.update_window(handle.into(), |_, _, cx| {
        cx.subscribe(
            &view,
            move |_, link: &crate::terminal::links::OpenLink, _| {
                *result.lock() = Some(link.clone())
            },
        )
    })?;
    let position = view.read_with(cx, |view, _| {
        view.bounds.origin + point(px(view.cell_width * 3.25), px(view.line_height * 0.5))
    });
    let control = Modifiers {
        control: true,
        ..Default::default()
    };
    writes.lock().clear();
    send(
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            modifiers: control,
            ..Default::default()
        }),
        cx,
    )?;
    send(
        PlatformInput::MouseDown(MouseDownEvent {
            position,
            modifiers: control,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    send(
        PlatformInput::MouseUp(MouseUpEvent {
            position,
            modifiers: control,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while received.lock().is_none() {
        anyhow::ensure!(Instant::now() < deadline, "Ctrl-click did not emit link");
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
    anyhow::ensure!(
        writes.lock().is_empty(),
        "link click leaked mouse reports to TUI"
    );
    anyhow::ensure!(
        matches!(&received.lock().as_ref().unwrap().target, crate::terminal::links::LinkTarget::Url(url) if url.as_str() == "https://example.org/"),
        "Ctrl-click emitted wrong link"
    );
    send(
        PlatformInput::MouseDown(MouseDownEvent {
            position,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    send(
        PlatformInput::MouseUp(MouseUpEvent {
            position,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while view.read_with(cx, |view, _| view.interactions_in_flight) {
        anyhow::ensure!(
            Instant::now() < deadline,
            "TUI mouse reports did not finish"
        );
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
    let output = writes.lock().concat();
    anyhow::ensure!(
        output == b"\x1b[<0;4;1M\x1b[<0;4;1m",
        "ordinary TUI click changed reports: {output:?}"
    );
    cx.update_window(handle.into(), |_, window, _| window.remove_window())?;
    Ok(())
}

#[cfg(debug_assertions)]
async fn wait_search(
    view: &Entity<TerminalView>,
    count: usize,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(4);
    while !view.read_with(cx, |view, _| {
        view.search
            .as_ref()
            .is_some_and(|search| search.result.matches.len() == count && !search.busy)
    }) {
        anyhow::ensure!(
            Instant::now() < deadline,
            "search did not settle at {count} matches"
        );
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
    Ok(())
}

fn tool(id: &'static str, icon: IconName, tooltip: impl Into<SharedString>) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .icon(icon)
        .size(px(26.))
        .rounded(px(4.))
        .tooltip(tooltip)
}

impl TerminalView {
    pub(super) fn active_input_focus(&self, cx: &App) -> FocusHandle {
        self.search.as_ref().map_or_else(
            || self.focus.clone(),
            |search| search.input.read(cx).focus_handle(cx),
        )
    }

    #[cfg(debug_assertions)]
    pub(crate) fn check_search_input(&self, window: &Window, cx: &App) -> Option<(String, bool)> {
        let input = self.search.as_ref()?.input.read(cx);
        Some((
            input.value().to_string(),
            input.focus_handle(cx).is_focused(window),
        ))
    }

    pub(crate) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::t!("terminal_search.placeholder"))
            });
            let subscription =
                cx.subscribe_in(&input, window, |this, _, event, _, cx| match event {
                    InputEvent::Change => {
                        if let Some(search) = &mut this.search {
                            search.query.text = search.input.read(cx).value().to_string();
                            search.generation += 1;
                            search.result = SearchResult::default();
                        }
                        this.refresh_search(cx);
                    }
                    InputEvent::PressEnter { shift, .. } => {
                        this.navigate_search(if *shift { -1 } else { 1 }, cx)
                    }
                    _ => {}
                });
            self.search = Some(SearchUi {
                input,
                query: SearchQuery::default(),
                result: SearchResult::default(),
                generation: 0,
                pending: false,
                busy: false,
                navigation: None,
                _subscription: subscription,
            });
        }
        if let Some(search) = &self.search {
            search.input.update(cx, |input, cx| input.focus(window, cx));
        }
        self.refresh_search(cx);
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_task = None;
        self.search = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn navigate_search(&mut self, direction: i8, cx: &mut Context<Self>) {
        if let Some(search) = &mut self.search {
            search.navigation = Some(direction);
        }
        self.refresh_search(cx);
    }

    fn set_search_option(&mut self, case: bool, cx: &mut Context<Self>) {
        if let Some(search) = &mut self.search {
            if case {
                search.query.case_sensitive = !search.query.case_sensitive;
            } else {
                search.query.regex = !search.query.regex;
            }
            search.generation += 1;
            search.result = SearchResult::default();
        }
        self.refresh_search(cx);
    }

    pub(super) fn refresh_search(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &mut self.search else {
            return;
        };
        search.pending = true;
        if search.busy {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        search.busy = true;
        self.search_task = Some(cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let Ok(Some((query, generation, current, navigation))) =
                    view.update(cx, |this, _| {
                        let search = this.search.as_mut()?;
                        search.pending = false;
                        let current = search
                            .result
                            .current
                            .and_then(|index| search.result.matches.get(index))
                            .map(|m| (*m.start(), search.result.content_revision));
                        Some((
                            search.query.clone(),
                            search.generation,
                            current,
                            search.navigation.take(),
                        ))
                    })
                else {
                    break;
                };
                let source = session.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { source.search_output(&query, current, navigation) })
                    .await;
                let Ok(true) = view.update(cx, |this, cx| {
                    let Some(search) = &mut this.search else {
                        return false;
                    };
                    if search.generation == generation {
                        search.result = result;
                        cx.notify();
                    }
                    if search.pending {
                        true
                    } else {
                        search.busy = false;
                        false
                    }
                }) else {
                    break;
                };
            }
        }));
    }

    pub(super) fn paint_search(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let Some(search) = &self.search else {
            return;
        };
        if search.result.revision != self.snapshot.revision {
            return;
        }
        let top = -(self.snapshot.display_offset as i32);
        let bottom = top + self.snapshot.rows.len() as i32;
        let first = search
            .result
            .matches
            .partition_point(|found| found.end().line.0 < top);
        for (index, found) in search
            .result
            .matches
            .iter()
            .enumerate()
            .skip(first)
            .take_while(|(_, found)| found.start().line.0 < bottom)
        {
            let range = SelectionRange::new(*found.start(), *found.end(), false);
            let mut spans = selection_spans(
                Some(range),
                self.snapshot.rows.len(),
                self.snapshot.cols,
                self.snapshot.display_offset,
                self.palette.selection,
            );
            expand_wide_selection(&mut spans, &self.snapshot.rows);
            for span in &spans {
                self.paint_span(bounds, span.row, span, window);
                if search.result.current == Some(index) {
                    let origin = bounds.origin
                        + point(
                            px(span.col as f32 * self.cell_width),
                            px(span.row as f32 * self.line_height),
                        );
                    let width = px(span.columns as f32 * self.cell_width);
                    for edge in [
                        Bounds::new(origin, size(width, px(1.))),
                        Bounds::new(
                            origin + point(px(0.), px(self.line_height - 1.)),
                            size(width, px(1.)),
                        ),
                        Bounds::new(origin, size(px(1.), px(self.line_height))),
                        Bounds::new(
                            origin + point(width - px(1.), px(0.)),
                            size(px(1.), px(self.line_height)),
                        ),
                    ] {
                        window.paint_quad(fill(edge, rgb(self.palette.accent)));
                    }
                }
            }
        }
    }

    pub(super) fn search_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = self.search.as_ref()?;
        let result = &search.result;
        let count = if result.invalid {
            crate::t!("terminal_search.invalid").to_string()
        } else {
            crate::t!(
                "terminal_search.count",
                current = result.current.map_or(0, |i| i + 1),
                count = format!(
                    "{}{}",
                    result.matches.len(),
                    if result.truncated { "+" } else { "" }
                )
            )
            .to_string()
        };
        Some(
            div()
                .id("terminal-search")
                .absolute()
                .top(px(6.))
                .right(px(6.))
                .w(px(316.))
                .max_w(px((f32::from(self.bounds.size.width) - 12.).max(0.)))
                .flex()
                .flex_col()
                .gap_1()
                .p_1()
                .rounded(px(4.))
                .bg(rgb(self.palette.panel))
                .border_1()
                .border_color(rgb(self.palette.border))
                .text_size(px(12.))
                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.close_search(window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&search.input).small()),
                        )
                        .child(
                            tool(
                                "terminal-search-close",
                                IconName::X,
                                crate::t!("terminal_search.close"),
                            )
                            .on_click(cx.listener(Self::close_search_click)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(62.))
                                .px_1()
                                .text_color(rgb(if result.invalid {
                                    self.palette.error
                                } else {
                                    self.palette.muted
                                }))
                                .child(count),
                        )
                        .child(
                            Button::new("terminal-search-case")
                                .ghost()
                                .small()
                                .label("Aa")
                                .h(px(26.))
                                .rounded(px(4.))
                                .selected(search.query.case_sensitive)
                                .tooltip(crate::t!("terminal_search.case"))
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.set_search_option(true, cx)),
                                ),
                        )
                        .child(
                            Button::new("terminal-search-regex")
                                .ghost()
                                .small()
                                .label(".*")
                                .h(px(26.))
                                .rounded(px(4.))
                                .selected(search.query.regex)
                                .tooltip(crate::t!("terminal_search.regex"))
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.set_search_option(false, cx)),
                                ),
                        )
                        .child(
                            tool(
                                "terminal-search-previous",
                                IconName::ChevronUp,
                                crate::t!("terminal_search.previous"),
                            )
                            .disabled(result.matches.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.navigate_search(-1, cx))),
                        )
                        .child(
                            tool(
                                "terminal-search-next",
                                IconName::ChevronDown,
                                crate::t!("terminal_search.next"),
                            )
                            .disabled(result.matches.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.navigate_search(1, cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    fn close_search_click(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.close_search(window, cx);
    }
}
