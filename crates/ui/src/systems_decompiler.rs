//! Snapshot-bound native pseudocode and machine-location navigation.
use super::*;
use crate::decompiler::{self, Decompiled};
use std::sync::atomic::{AtomicBool, Ordering};

const PAGE_LINES: usize = 32;
const ROW_BYTES: usize = 256;

struct DisplayRow {
    range: std::ops::Range<usize>,
    source_line: usize,
    continuation: bool,
}

// Bound both shaping work and token elements, even for output without newlines.
fn display_rows(text: &str) -> Vec<DisplayRow> {
    let mut rows = Vec::new();
    let mut start = 0;
    for (source_line, line) in text.split_inclusive('\n').enumerate() {
        let end = start + line.trim_end_matches('\n').len();
        let mut cursor = start;
        loop {
            let mut stop = (cursor + ROW_BYTES).min(end);
            while !text.is_char_boundary(stop) {
                stop -= 1;
            }
            rows.push(DisplayRow {
                range: cursor..stop,
                source_line: source_line + 1,
                continuation: cursor != start,
            });
            if stop == end {
                break;
            }
            cursor = stop;
        }
        start += line.len();
    }
    rows
}
pub(super) struct DecompilerState {
    worker: Option<PathBuf>,
    result: Option<Decompiled>,
    entry: Option<u64>,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    running: bool,
    status: String,
    page: usize,
    lines: Vec<DisplayRow>,
}
impl Default for DecompilerState {
    fn default() -> Self {
        Self {
            worker: std::env::var_os("ALE_DECOMPILER").map(PathBuf::from),
            result: None,
            entry: None,
            generation: 0,
            cancel: None,
            running: false,
            status: "Select a function, then Decompile. An optional native worker is required."
                .into(),
            page: 0,
            lines: Vec::new(),
        }
    }
}
impl Drop for DecompilerState {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}
impl Workbench {
    fn set_pseudocode_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let last = self.decompiler.lines.len().saturating_sub(1) / PAGE_LINES;
        let page = page.min(last);
        if page != self.decompiler.page {
            self.decompiler.page = page;
            self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            cx.notify();
        }
    }

    pub(super) fn invalidate_decompiler(&mut self) {
        if let Some(cancel) = self.decompiler.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.decompiler.generation = self.decompiler.generation.wrapping_add(1);
        self.decompiler.running = false;
        self.decompiler.result = None;
        self.decompiler.entry = None;
        self.decompiler.lines.clear();
        self.decompiler.page = 0;
        self.decompiler.status =
            "Pseudocode cleared; decompile the selected function again.".into();
    }
    pub(super) fn decompiler_location_changed(&mut self) {
        if self.decompiler.entry.is_some()
            && self.selected_function().map(|f| f.address) != self.decompiler.entry
        {
            self.invalidate_decompiler();
        }
    }
    pub(super) fn start_decompiler(&mut self, cx: &mut Context<Self>) {
        self.invalidate_decompiler();
        let Some(worker) = self.decompiler.worker.clone() else {
            self.decompiler.status = "Native decompiler is not configured. Set ALE_DECOMPILER to an absolute worker path before launching; see tools/native-decompiler/README.md.".into();
            cx.notify();
            return;
        };
        let Some(entry) = self.selected_function().map(|function| function.address) else {
            self.decompiler.status = "Choose a recovered function after analysis finishes.".into();
            cx.notify();
            return;
        };
        let Some(analysis) = self.analysis.result.clone() else {
            return;
        };
        let image = self.image.clone();
        let revision = self.revision;
        let generation = self.decompiler.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.decompiler.cancel = Some(cancel.clone());
        self.decompiler.entry = Some(entry);
        self.decompiler.running = true;
        self.decompiler.status = "Decompiling captured bytes…".into();
        let work = cx.background_executor().spawn(async move {
            analysis
                .functions
                .iter()
                .find(|function| function.address == entry)
                .ok_or_else(|| "Captured function is unavailable".to_owned())
                .and_then(|function| {
                    decompiler::decompile(&worker, &image, &analysis, function, &cancel).map(
                        |result| {
                            let rows = display_rows(&result.text);
                            (result, rows)
                        },
                    )
                })
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.revision != revision || this.decompiler.generation != generation {
                    return;
                }
                this.decompiler.running = false;
                this.decompiler.cancel = None;
                match result {
                    Ok((result, rows)) => {
                        this.decompiler.status = format!(
                            "Native pseudocode · function {:x} · inferred types · {} diagnostics",
                            result.entry,
                            result.diagnostics.len()
                        );
                        this.decompiler.lines = rows;
                        this.decompiler.result = Some(result);
                    }
                    Err(error) => this.decompiler.status = format!("Decompilation failed: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn pseudocode_view(&self, cx: &Context<Self>) -> Div {
        let mut root = div().flex().flex_col().min_w_full().child(
            div()
                .flex()
                .items_center()
                .h(px(30.0))
                .px_2()
                .gap_2()
                .border_b_1()
                .border_color(rgb(0x2a313c))
                .child(
                    button(if self.decompiler.running {
                        "Cancel"
                    } else {
                        "Decompile"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.decompiler.running {
                            this.invalidate_decompiler();
                            this.decompiler.status =
                                "Decompilation cancelled; document retained.".into();
                            cx.notify();
                        } else {
                            this.start_decompiler(cx);
                        }
                    })),
                )
                .child(button("Copy code").on_click(cx.listener(|this, _, _, cx| {
                    if let Some(result) = &this.decompiler.result {
                        cx.write_to_clipboard(ClipboardItem::new_string(result.text.clone()));
                    }
                })))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(0x8590a3))
                        .child(self.decompiler.status.clone()),
                ),
        );
        let Some(result) = &self.decompiler.result else {
            return root.child(
                div()
                    .p_4()
                    .whitespace_normal()
                    .text_color(rgb(0x8590a3))
                    .child(self.decompiler.status.clone()),
            );
        };
        root = root.child(div().px_3().py_2().text_size(px(11.0)).text_color(rgb(0x8590a3))
            .child("Recovered pseudocode, not original source. Click a mapped token to select its machine location; use Assembly or Bytes to inspect it."));
        for (line_index, row) in self
            .decompiler
            .lines
            .iter()
            .enumerate()
            .skip(self.decompiler.page * PAGE_LINES)
            .take(PAGE_LINES)
        {
            let range = &row.range;
            let mut line = div()
                .id(("pseudocode-line", line_index))
                .flex()
                .h(px(self.font_size + 7.0))
                .font_family("DejaVu Sans Mono")
                .text_size(px(self.font_size))
                .px_2()
                .child(
                    div()
                        .w(px(42.0))
                        .flex_shrink_0()
                        .text_color(rgb(0x59677c))
                        .child(if row.continuation {
                            "↳".to_owned()
                        } else {
                            row.source_line.to_string()
                        }),
                );
            let first = result
                .tokens
                .partition_point(|token| token.range.end <= range.start);
            let mut cursor = range.start;
            for (index, token) in result
                .tokens
                .iter()
                .enumerate()
                .skip(first)
                .take_while(|(_, token)| token.range.start < range.end)
            {
                let start = token.range.start.max(range.start);
                let end = token.range.end.min(range.end);
                if cursor < start {
                    line = line.child(
                        div()
                            .flex_shrink_0()
                            .child(result.text[cursor..start].to_owned()),
                    );
                }
                let mut span = div()
                    .id(("pseudocode-token", index))
                    .debug_selector(|| format!("pseudocode-token-{line_index}-{index}"))
                    .flex_shrink_0()
                    .text_color(rgb(match token.kind.as_str() {
                        "type" => 0x7dcfff,
                        "syntax" => 0x9d7cd8,
                        "funcname" => 0x7aa2f7,
                        _ => 0xd4dce7,
                    }))
                    .child(result.text[start..end].to_owned());
                if let Some(offset) = token.offset {
                    span = span
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0x273b59)))
                        .when(self.offset == offset, |span| span.bg(rgb(0x273b59)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let scroll = this.scroll.offset();
                            this.go(offset, cx);
                            this.scroll.set_offset(scroll);
                        }));
                }
                line = line.child(span);
                cursor = end;
            }
            if cursor < range.end {
                line = line.child(
                    div()
                        .flex_shrink_0()
                        .child(result.text[cursor..range.end].to_owned()),
                );
            }
            root = root.child(line);
        }
        root = root.child(
            div()
                .flex()
                .p_2()
                .gap_2()
                .child(
                    button("Previous rows").on_click(cx.listener(|this, _, _, cx| {
                        this.set_pseudocode_page(this.decompiler.page.saturating_sub(1), cx);
                    })),
                )
                .child(button("Next rows").on_click(cx.listener(|this, _, _, cx| {
                    this.set_pseudocode_page(this.decompiler.page.saturating_add(1), cx);
                })))
                .child(format!(
                    "Page {} · {} display rows",
                    self.decompiler.page + 1,
                    self.decompiler.lines.len()
                )),
        );
        root.children(result.diagnostics.iter().map(|diagnostic| {
            div()
                .px_3()
                .py_1()
                .whitespace_normal()
                .text_color(rgb(0xe0af68))
                .child(diagnostic.clone())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[test]
    fn display_rows_bound_long_unicode_output_without_losing_text() {
        let text = format!("{}\n\nlast", "a🦀e\u{301}".repeat(100_000));
        let rows = display_rows(&text);
        assert!(rows.len() > PAGE_LINES);
        for row in &rows {
            assert!(row.range.len() <= ROW_BYTES);
            assert!(text.is_char_boundary(row.range.start));
            assert!(text.is_char_boundary(row.range.end));
        }
        let first_line: String = rows
            .iter()
            .filter(|r| r.source_line == 1)
            .map(|r| &text[r.range.clone()])
            .collect();
        assert_eq!(first_line, text.split('\n').next().unwrap());
        assert!(!rows[0].continuation);
        assert!(rows[1].continuation);
        let blank = rows.iter().find(|r| r.source_line == 2).unwrap();
        assert!(blank.range.is_empty());
        assert!(!blank.continuation);
        assert_eq!(&text[rows.last().unwrap().range.clone()], "last");
    }

    #[test]
    fn display_rows_keep_exact_boundaries_and_empty_output() {
        assert!(display_rows("").is_empty());
        let text = format!("{}\nnext\n", "x".repeat(ROW_BYTES));
        let rows = display_rows(&text);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].range, 0..ROW_BYTES);
        assert_eq!(rows[1].source_line, 2);
        assert_eq!(&text[rows[1].range.clone()], "next");
    }

    #[gpui::test]
    fn wrapped_token_click_keeps_mapping_across_pages(cx: &mut TestAppContext) {
        let (image, _) = crate::decompiler::tests::fixture();
        let (view, cx) =
            cx.add_window_view(|_, cx| Workbench::new(PathBuf::from("fixture.elf"), image, cx));
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.go(0x100, cx);
                view.lens = Lens::Pseudocode;
                let text = "é🦀".repeat(2000);
                view.decompiler.lines = display_rows(&text);
                view.decompiler.entry = Some(0x401000);
                view.decompiler.result = Some(Decompiled {
                    entry: 0x401000,
                    tokens: vec![decompiler::Token {
                        range: 0..text.len(),
                        offset: Some(0x103),
                        kind: "variable".into(),
                    }],
                    text,
                    diagnostics: Vec::new(),
                });
            });
        });
        for (page, selector) in [(0, "pseudocode-token-0-0"), (1, "pseudocode-token-32-0")] {
            cx.update(|_, cx| {
                view.update(cx, |view, _| {
                    view.decompiler.page = page;
                    view.offset = 0x100;
                })
            });
            cx.draw(
                gpui::point(px(0.0), px(0.0)),
                gpui::size(px(1000.0), px(800.0)),
                |_, _| view.clone().into_any_element(),
            );
            let bounds = cx
                .debug_bounds(selector)
                .expect("wrapped token was painted");
            cx.simulate_click(bounds.center(), gpui::Modifiers::none());
            cx.update(|_, cx| {
                view.update(cx, |view, _| {
                    assert_eq!(view.offset, 0x103);
                    assert!(view.decompiler.result.is_some());
                    assert_eq!(view.decompiler.page, page);
                })
            });
        }
    }

    #[gpui::test]
    fn page_change_reveals_start_and_preserves_machine_location(cx: &mut TestAppContext) {
        let (image, _) = crate::decompiler::tests::fixture();
        let (view, cx) =
            cx.add_window_view(|_, cx| Workbench::new(PathBuf::from("fixture.elf"), image, cx));
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.decompiler.lines = display_rows(&"line\n".repeat(PAGE_LINES + 1));
                view.offset = 0x103;
                let scrolled = gpui::point(px(-80.0), px(-120.0));
                view.scroll.set_offset(scrolled);
                view.set_pseudocode_page(1, cx);
                assert_eq!(view.scroll.offset(), gpui::point(px(0.0), px(0.0)));
                assert_eq!(view.offset, 0x103);
                view.scroll.set_offset(scrolled);
                view.set_pseudocode_page(usize::MAX, cx);
                assert_eq!(view.decompiler.page, 1);
                assert_eq!(
                    view.scroll.offset(),
                    scrolled,
                    "last-page no-op retains scroll"
                );
                view.set_pseudocode_page(0, cx);
                assert_eq!(view.scroll.offset(), gpui::point(px(0.0), px(0.0)));
                assert_eq!(view.offset, 0x103);
            })
        });
    }

    #[gpui::test]
    fn native_result_tracks_snapshot_location_and_cancellation(cx: &mut TestAppContext) {
        let Some(worker) = std::env::var_os("ALE_DECOMPILER_TEST") else {
            eprintln!("SKIP: configured native worker required for pseudocode UI acceptance");
            return;
        };
        let (image, _) = crate::decompiler::tests::fixture();
        let (view, cx) =
            cx.add_window_view(|_, cx| Workbench::new(PathBuf::from("fixture.elf"), image, cx));
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.decompiler.worker = Some(worker.into());
                view.lens = Lens::Pseudocode;
                view.start_decompiler(cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let result = view
                    .decompiler
                    .result
                    .as_ref()
                    .expect(&view.decompiler.status);
                assert!(result.text.contains("+ 1"));
                let offset = result.tokens.iter().find_map(|token| token.offset).unwrap();
                view.go(offset, cx);
                assert!(view.decompiler.result.is_some());
                // A cancelled generation must never publish its eventual result.
                view.start_decompiler(cx);
                view.invalidate_decompiler();
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert!(view.decompiler.result.is_none());
                assert!(!view.decompiler.running);
                view.start_decompiler(cx);
                // Model the same snapshot replacement/invalidation used by Apply.
                view.image = Arc::new(view.image.patched(0x102, &[2]).unwrap());
                view.revision += 1;
                view.refresh_analysis(cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert!(view.decompiler.result.is_none());
                view.start_decompiler(cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert!(
                    view.decompiler
                        .result
                        .as_ref()
                        .expect(&view.decompiler.status)
                        .text
                        .contains("+ 2")
                );
                view.go(0, cx);
                assert!(
                    view.decompiler.result.is_none(),
                    "A different function/location clears old output"
                );
            })
        });
    }
}
