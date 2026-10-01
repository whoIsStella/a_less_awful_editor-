//! Snapshot-bound analysis jobs and native control-flow presentation.
use super::*;
use ale_systems_core::{Analysis, AnalysisLimits, AnalyzedFunction, EdgeKind, EdgeResolution};
use gpui::{PathBuilder, canvas, point};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(super) struct AnalysisState {
    pub result: Option<Arc<Analysis>>,
    pub running: bool,
    pub status: String,
    generation: u64,
    cancellation: Option<Arc<AtomicBool>>,
    focused_function: Option<usize>,
    symbols: bool,
    references: bool,
}

impl Drop for AnalysisState {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancellation {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Workbench {
    pub(super) fn refresh_analysis(&mut self, cx: &mut Context<Self>) {
        self.cancel_analysis(cx);
        self.analysis.result = None;
        self.analysis.running = true;
        self.analysis.status = "Discovering functions and control flow…".into();
        let generation = self.analysis.generation;
        let revision = self.revision;
        let image = self.image.clone();
        let architecture = self.architecture.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.analysis.cancellation = Some(cancel.clone());
        let work = cx
            .background_executor()
            .spawn(async move { image.analyze(&architecture, AnalysisLimits::default(), &cancel) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.analysis.generation != generation || this.revision != revision {
                    return;
                }
                this.analysis.running = false;
                this.analysis.cancellation = None;
                match result {
                    Ok(result) => {
                        this.analysis.status = format!(
                            "{} function candidates · {} references{}",
                            result.functions.len(),
                            result.references.len(),
                            if result.truncated {
                                " · analysis limit reached"
                            } else {
                                ""
                            }
                        );
                        this.analysis.result = Some(Arc::new(result));
                    }
                    Err(error) => this.analysis.status = error,
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn cancel_analysis(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.analysis.cancellation.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.analysis.generation = self.analysis.generation.wrapping_add(1);
        self.analysis.running = false;
        self.analysis.status = "Analysis cancelled. The document is unchanged.".into();
        cx.notify();
    }

    pub(super) fn selected_function(&self) -> Option<&AnalyzedFunction> {
        let analysis = self.analysis.result.as_ref()?;
        let contains = |function: &&AnalyzedFunction| {
            function.blocks.iter().any(|block| {
                block.instructions.iter().any(|instruction| {
                    self.offset >= instruction.offset
                        && self.offset < instruction.offset.saturating_add(instruction.bytes.len())
                })
            })
        };
        analysis
            .functions
            .iter()
            .find(|function| {
                Some(function.offset) == self.analysis.focused_function && contains(function)
            })
            .or_else(|| analysis.functions.iter().find(contains))
    }

    pub(super) fn analysis_sidebar(&self, cx: &Context<Self>) -> Div {
        let functions = self
            .analysis
            .result
            .as_ref()
            .map(|analysis| &analysis.functions);
        let selected = self.selected_function().map(|function| function.offset);
        div()
            .w(px(168.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(0x15191f))
            .border_r_1()
            .border_color(rgb(0x2a313c))
            .child(
                div()
                    .flex()
                    .h(px(26.0))
                    .flex_shrink_0()
                    .items_center()
                    .px_1()
                    .child(
                        button("Functions")
                            .text_size(px(11.0))
                            .when(!self.analysis.symbols, |tab| tab.text_color(rgb(0x7aa2f7)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.analysis.symbols = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("Symbols")
                            .text_size(px(11.0))
                            .when(self.analysis.symbols, |tab| tab.text_color(rgb(0x7aa2f7)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.analysis.symbols = true;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("analysis-symbol-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(!self.analysis.symbols, |list| {
                        if let Some(functions) = functions {
                            list.children(functions.iter().enumerate().map(|(index, function)| {
                                let offset = function.offset;
                                div()
                                    .id(("function", index))
                                    .h(px(22.0))
                                    .flex()
                                    .items_center()
                                    .px_3()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x1c2430)))
                                    .text_size(px(11.5))
                                    .truncate()
                                    .text_color(rgb(if selected == Some(offset) {
                                        0x7aa2f7
                                    } else {
                                        0x8590a3
                                    }))
                                    .when(selected == Some(offset), |row| row.bg(rgb(0x1c2430)))
                                    .child(function.name.clone())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.analysis.focused_function = Some(offset);
                                        this.go(offset, cx);
                                        if this.lens != Lens::Flow {
                                            this.lens = Lens::Assembly;
                                        }
                                    }))
                            }))
                        } else {
                            list.child(
                                div()
                                    .p_3()
                                    .text_size(px(11.0))
                                    .text_color(rgb(0x8590a3))
                                    .whitespace_normal()
                                    .child(self.analysis.status.clone()),
                            )
                        }
                    })
                    .when(self.analysis.symbols, |list| {
                        list.children(
                            self.image
                                .symbols
                                .iter()
                                .take(256)
                                .filter_map(|symbol| {
                                    symbol
                                        .file_offset
                                        .map(|offset| (offset, symbol.name.clone()))
                                })
                                .enumerate()
                                .map(|(index, (offset, name))| {
                                    div()
                                        .id(("analysis-symbol", index))
                                        .h(px(22.0))
                                        .flex()
                                        .items_center()
                                        .px_3()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0x1c2430)))
                                        .text_size(px(11.5))
                                        .truncate()
                                        .text_color(rgb(0x8590a3))
                                        .child(name)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.go(offset, cx);
                                            this.lens = Lens::Assembly;
                                        }))
                                }),
                        )
                    }),
            )
    }

    pub(super) fn flow_view(&self, cx: &Context<Self>) -> Div {
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
                    button("Blocks")
                        .text_size(px(11.0))
                        .when(!self.analysis.references, |tab| {
                            tab.text_color(rgb(0x7aa2f7))
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.analysis.references = false;
                            cx.notify();
                        })),
                )
                .child(
                    button("References")
                        .text_size(px(11.0))
                        .when(self.analysis.references, |tab| {
                            tab.text_color(rgb(0x7aa2f7))
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.analysis.references = true;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.0))
                        .text_color(rgb(0x8590a3))
                        .child(self.analysis.status.clone()),
                )
                .child(
                    button(if self.analysis.running {
                        "Cancel"
                    } else {
                        "Analyze"
                    })
                    .text_size(px(11.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.analysis.running {
                            this.cancel_analysis(cx);
                        } else {
                            this.refresh_analysis(cx);
                        }
                    })),
                ),
        );
        if self.analysis.references {
            return root.child(self.references_view(cx));
        }
        let Some(function) = self.selected_function() else {
            return root.child(div().p_4().text_size(px(12.0)).text_color(rgb(0x8590a3)).whitespace_normal()
                .child("No discovered function contains this location. Choose a function from the sidebar. Indirect targets and unknown regions remain unresolved."));
        };
        root = root.child(
            div()
                .px_3()
                .py_2()
                .text_size(px(11.0))
                .text_color(rgb(0x8590a3))
                .child(format!(
                    "{} · {:x} · {:?} · {} blocks · branch / next / call",
                    function.name,
                    function.address,
                    function.provenance,
                    function.blocks.len()
                )),
        );
        let layout = graph_layout(function);
        let routes = layout.routes.clone();
        let mut graph = div()
            .relative()
            .w(px(layout.width))
            .h(px(layout.height))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        for route in routes {
                            let mut path = PathBuilder::stroke(px(1.25));
                            for (index, (x, y)) in route.points.iter().enumerate() {
                                let position = bounds.origin + point(px(*x), px(*y));
                                if index == 0 {
                                    path.move_to(position);
                                } else {
                                    path.line_to(position);
                                }
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, rgb(route.color));
                            }
                            if let Some(&(x, y)) = route.points.last() {
                                let mut arrow = PathBuilder::stroke(px(1.25));
                                arrow.move_to(bounds.origin + point(px(x - 3.0), px(y - 5.0)));
                                arrow.line_to(bounds.origin + point(px(x), px(y)));
                                arrow.line_to(bounds.origin + point(px(x + 3.0), px(y - 5.0)));
                                if let Ok(path) = arrow.build() {
                                    window.paint_path(path, rgb(route.color));
                                }
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        for (index, node) in layout.nodes.iter().enumerate() {
            let block = &function.blocks[index];
            let start = block.start_offset;
            let selected = block.instructions.iter().any(|instruction| {
                self.offset >= instruction.offset
                    && self.offset < instruction.offset + instruction.bytes.len()
            });
            let mut card = div()
                .absolute()
                .left(px(node.x))
                .top(px(node.y))
                .w(px(NODE_WIDTH))
                .h(px(node.height))
                .bg(rgb(0x15191f))
                .border_1()
                .border_color(rgb(if selected { 0x7aa2f7 } else { 0x2a313c }))
                .rounded_sm()
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(26.0))
                        .px_2()
                        .bg(rgb(0x1b2028))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(0x7aa2f7))
                                .child(format!("BLOCK {:08x}", block.start_address)),
                        )
                        .child(button("Listing").text_size(px(10.0)).h(px(22.0)).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.go(start, cx);
                                this.lens = Lens::Assembly;
                            }),
                        )),
                );
            for instruction in block.instructions.iter().take(NODE_LINES) {
                let offset = instruction.offset;
                let selected =
                    self.offset >= offset && self.offset < offset + instruction.bytes.len();
                card = card.child(
                    div()
                        .id(("block-instruction", offset))
                        .h(px(20.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .font_family("DejaVu Sans Mono")
                        .text_size(px(11.0))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0x1c2430)))
                        .when(selected, |row| row.bg(rgb(0x273b59)))
                        .child(
                            div()
                                .w(px(66.0))
                                .flex_shrink_0()
                                .text_color(rgb(0x8590a3))
                                .child(format!("{:08x}", instruction.address)),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(rgb(0xd4dce7))
                                .child(instruction.text.clone()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let scroll = this.scroll.offset();
                            this.go(offset, cx);
                            this.scroll.set_offset(scroll);
                        })),
                );
            }
            if block.instructions.len() > NODE_LINES {
                card = card.child(
                    div()
                        .px_2()
                        .h(px(20.0))
                        .text_size(px(10.0))
                        .text_color(rgb(0x8590a3))
                        .child(format!(
                            "{} more instructions in Listing",
                            block.instructions.len() - NODE_LINES
                        )),
                );
            }
            for (edge_index, edge) in block.edges.iter().enumerate() {
                let target = edge.target_offset;
                let label = format!(
                    "{:?} {}{}",
                    edge.kind,
                    edge.target_address
                        .map(|address| format!("{address:x}"))
                        .unwrap_or_default(),
                    if edge.resolution == EdgeResolution::Resolved {
                        String::new()
                    } else {
                        format!(" · {:?}", edge.resolution)
                    }
                );
                card = card.child(
                    div()
                        .id(("block-edge", edge_index))
                        .h(px(19.0))
                        .px_2()
                        .text_size(px(10.0))
                        .text_color(rgb(edge_color(edge.kind)))
                        .truncate()
                        .child(label)
                        .when_some(target, |row, offset| {
                            row.cursor_pointer()
                                .hover(|style| style.bg(rgb(0x1c2430)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.go(offset, cx);
                                }))
                        }),
                );
            }
            graph = graph.child(card);
        }
        root.child(graph).when(function.blocks.len() > MAX_NODES, |root| root.child(
            div().p_3().text_size(px(11.0)).text_color(rgb(0xe0af68)).child("Graph shows the first 64 blocks; use Listing for the remaining locations.")))
    }

    fn references_view(&self, cx: &Context<Self>) -> Div {
        let mut root = div().flex().flex_col().min_w_full();
        let Some(analysis) = &self.analysis.result else {
            return root;
        };
        let function = self.selected_function();
        let instruction_offsets: BTreeSet<_> = function
            .into_iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.instructions)
            .map(|instruction| instruction.offset)
            .collect();
        let in_function = |offset: usize| instruction_offsets.contains(&offset);
        let matches: Vec<_> = analysis
            .references
            .iter()
            .filter(|reference| {
                reference.from_offset == self.offset
                    || reference.target_offset == Some(self.offset)
                    || in_function(reference.from_offset)
                    || reference.target_offset.is_some_and(in_function)
            })
            .take(256)
            .collect();
        root = root.child(
            div()
                .p_3()
                .text_size(px(11.0))
                .text_color(rgb(0x8590a3))
                .child(format!(
                    "{} references touching this function/location (up to 256)",
                    matches.len()
                )),
        );
        for (index, reference) in matches.into_iter().enumerate() {
            let from = reference.from_offset;
            let target = reference.target_offset;
            root = root.child(
                div()
                    .id(("reference", index))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(0x1b2028))
                    .child(
                        button(format!("{:08x}", reference.from_address))
                            .font_family("DejaVu Sans Mono")
                            .text_color(rgb(0x7aa2f7))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.go(from, cx);
                                this.lens = Lens::Assembly;
                            })),
                    )
                    .child(
                        div()
                            .w(px(110.0))
                            .text_size(px(11.0))
                            .text_color(rgb(0x8590a3))
                            .child(format!("{:?}", reference.kind)),
                    )
                    .child(
                        button(
                            reference
                                .target_address
                                .map(|address| format!("{address:08x}"))
                                .unwrap_or_else(|| "unresolved".into()),
                        )
                        .font_family("DejaVu Sans Mono")
                        .when_some(target, |button, target| {
                            button.text_color(rgb(0x7aa2f7)).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.go(target, cx);
                                    this.lens = Lens::Assembly;
                                },
                            ))
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(0x8590a3))
                            .child(format!("{:?}", reference.resolution)),
                    ),
            );
        }
        root
    }
}

const MAX_NODES: usize = 64;
const NODE_LINES: usize = 6;
const NODE_WIDTH: f32 = 320.0;
struct Node {
    x: f32,
    y: f32,
    height: f32,
}
#[derive(Clone)]
struct Route {
    points: Vec<(f32, f32)>,
    color: u32,
}
struct GraphLayout {
    nodes: Vec<Node>,
    routes: Vec<Route>,
    width: f32,
    height: f32,
}
fn edge_color(kind: EdgeKind) -> u32 {
    match kind {
        EdgeKind::ConditionalBranch => 0x9ece6a,
        EdgeKind::Call | EdgeKind::IndirectCall => 0xe0af68,
        EdgeKind::Fallthrough => 0x59677c,
        _ => 0x7aa2f7,
    }
}

fn graph_layout(function: &AnalyzedFunction) -> GraphLayout {
    let blocks = &function.blocks[..function.blocks.len().min(MAX_NODES)];
    let indices: BTreeMap<_, _> = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.start_offset, index))
        .collect();
    let mut ranks = vec![None; blocks.len()];
    let mut queue = VecDeque::new();
    if !blocks.is_empty() {
        ranks[0] = Some(0usize);
        queue.push_back(0);
    }
    while let Some(index) = queue.pop_front() {
        let rank = ranks[index].unwrap();
        for edge in &blocks[index].edges {
            if matches!(edge.kind, EdgeKind::Call | EdgeKind::IndirectCall) {
                continue;
            }
            if let Some(target) = edge
                .target_offset
                .and_then(|offset| indices.get(&offset))
                .copied()
                && ranks[target].is_none()
            {
                ranks[target] = Some(rank + 1);
                queue.push_back(target);
            }
        }
    }
    let mut layers: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let last = ranks.iter().flatten().copied().max().unwrap_or(0) + 1;
    for (index, rank) in ranks.iter().enumerate() {
        layers
            .entry(rank.unwrap_or(last + index))
            .or_default()
            .push(index);
    }
    let mut nodes: Vec<Node> = blocks
        .iter()
        .map(|block| Node {
            x: 0.0,
            y: 0.0,
            height: 28.0
                + block.instructions.len().min(NODE_LINES) as f32 * 20.0
                + if block.instructions.len() > NODE_LINES {
                    20.0
                } else {
                    0.0
                }
                + block.edges.len() as f32 * 19.0,
        })
        .collect();
    let mut y = 22.0;
    let mut width: f32 = NODE_WIDTH + 48.0;
    for layer in layers.values() {
        // Wrap very wide ranks; every block remains independently navigable.
        for row in layer.chunks(3) {
            let height = row
                .iter()
                .map(|index| nodes[*index].height)
                .fold(0.0_f32, f32::max);
            for (column, index) in row.iter().enumerate() {
                nodes[*index].x = 24.0 + column as f32 * (NODE_WIDTH + 40.0);
                nodes[*index].y = y;
                width = width.max(nodes[*index].x + NODE_WIDTH + 24.0);
            }
            y += height + 48.0;
        }
    }
    let mut routes = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        for edge in &block.edges {
            if matches!(edge.kind, EdgeKind::Call | EdgeKind::IndirectCall) {
                continue;
            }
            let Some(target) = edge
                .target_offset
                .and_then(|offset| indices.get(&offset))
                .copied()
            else {
                continue;
            };
            let from = &nodes[index];
            let to = &nodes[target];
            let start = (from.x + NODE_WIDTH / 2.0, from.y + from.height);
            let end = (to.x + NODE_WIDTH / 2.0, to.y);
            let points = if end.1 > start.1 {
                let middle = start.1 + 20.0;
                vec![start, (start.0, middle), (end.0, middle), end]
            } else {
                let lane = 8.0 + (index % 3) as f32 * 4.0;
                vec![
                    start,
                    (start.0, start.1 + 12.0),
                    (lane, start.1 + 12.0),
                    (lane, end.1 - 12.0),
                    (end.0, end.1 - 12.0),
                    end,
                ]
            };
            routes.push(Route {
                points,
                color: edge_color(edge.kind),
            });
        }
    }
    GraphLayout {
        nodes,
        routes,
        width,
        height: y,
    }
}
