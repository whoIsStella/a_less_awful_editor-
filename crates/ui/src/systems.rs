//! Native linked views of one immutable binary snapshot. No target code is run.
use crate::systems_io;
use ale_editor_view::EditorView;
use ale_systems_core::{Architecture, BinaryImage, Instruction, SourceLocation};
use gpui::{
    App, ClipboardItem, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    Render, Stateful, Window, div, prelude::*, px, rgb,
};
use std::{path::PathBuf, sync::Arc};

#[path = "systems_analysis.rs"]
mod analysis;
use analysis::AnalysisState;

#[cfg(test)]
#[path = "systems_tests.rs"]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lens {
    Overview,
    Assembly,
    Flow,
    Bytes,
    Strings,
}
impl Lens {
    fn name(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Assembly => "Assembly",
            Self::Flow => "Flow",
            Self::Bytes => "Bytes",
            Self::Strings => "Strings",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Go,
    Find,
    Assemble,
    Patch,
}
impl Command {
    fn name(self) -> &'static str {
        match self {
            Self::Go => "Go to",
            Self::Find => "Find",
            Self::Assemble => "Assemble",
            Self::Patch => "Patch bytes",
        }
    }
    fn help(self) -> &'static str {
        match self {
            Self::Go => "Hex virtual address, or @hex file offset",
            Self::Find => "Symbol name or printable string (Enter text, then Run)",
            Self::Assemble => {
                "One Intel instruction; preview first, then apply an equal-length patch"
            }
            Self::Patch => "Hex bytes separated by spaces; preview first, then apply",
        }
    }
}
pub(crate) enum WorkbenchEvent {
    Source(SourceLocation),
    Export,
}

pub(crate) struct Workbench {
    pub path: PathBuf,
    image: Arc<BinaryImage>,
    pub offset: usize,
    lens: Lens,
    architecture: Architecture,
    focus: FocusHandle,
    input: Entity<EditorView>,
    input_generation: u64,
    input_revision: u64,
    command: Command,
    message: String,
    preview: Option<(usize, Vec<u8>, u64)>,
    revision: u64,
    next_revision: u64,
    saved_revision: u64,
    undo: Vec<(u64, Arc<BinaryImage>)>,
    redo: Vec<(u64, Arc<BinaryImage>)>,
    back: Vec<usize>,
    instructions: Vec<Instruction>,
    decode_error: Option<String>,
    entropy: Vec<(usize, f64)>,
    matches: Vec<(usize, String)>,
    busy: bool,
    inspector: bool,
    details_visible: bool,
    row_bytes: usize,
    font_size: f32,
    scroll: gpui::ScrollHandle,
    analysis: AnalysisState,
}
impl EventEmitter<WorkbenchEvent> for Workbench {}
impl Focusable for Workbench {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

pub(crate) fn button(label: impl Into<gpui::SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(label.clone())
        .h(px(24.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .px_2()
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(rgb(0x8590a3))
        .hover(|style| style.bg(rgb(0x1c2430)).text_color(rgb(0xd4dce7)))
        .child(label)
}

impl Workbench {
    pub fn new(path: PathBuf, image: BinaryImage, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| EditorView::input("", cx));
        cx.subscribe(&input, |this, _, _: &ale_editor_view::Submit, cx| {
            this.run(cx)
        })
        .detach();
        cx.observe(&input, |this, input, cx| {
            let revision = input.read(cx).content_revision();
            if revision != this.input_revision {
                this.input_revision = revision;
                this.input_generation = this.input_generation.wrapping_add(1);
                this.preview = None;
                cx.notify();
            }
        })
        .detach();
        let offset = image
            .entry
            .and_then(|address| image.address_to_offset(address))
            .or_else(|| {
                image
                    .sections
                    .iter()
                    .find(|s| s.executable && s.file_size > 0)
                    .map(|s| s.file_offset)
            })
            .unwrap_or(0);
        let architecture = image.architecture.clone();
        let mut this = Self {
            path,
            image: Arc::new(image),
            offset,
            architecture,
            lens: Lens::Overview,
            focus: cx.focus_handle(),
            input,
            input_generation: 0,
            input_revision: 0,
            command: Command::Go,
            message: "Choose a section or symbol, then move between source, assembly, and bytes."
                .into(),
            preview: None,
            revision: 0,
            next_revision: 1,
            saved_revision: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            back: Vec::new(),
            instructions: Vec::new(),
            decode_error: None,
            entropy: Vec::new(),
            matches: Vec::new(),
            busy: false,
            inspector: true,
            details_visible: false,
            row_bytes: 16,
            font_size: 12.5,
            scroll: gpui::ScrollHandle::new(),
            analysis: AnalysisState::default(),
        };
        this.decode();
        this.refresh_entropy(cx);
        this.refresh_analysis(cx);
        this
    }
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }
    pub fn is_busy(&self) -> bool {
        self.busy
    }
    pub fn snapshot(&self) -> (Arc<BinaryImage>, u64) {
        (self.image.clone(), self.revision)
    }
    pub fn mark_exported(&mut self, revision: u64, cx: &mut Context<Self>) {
        self.saved_revision = revision;
        self.message = if self.is_dirty() {
            "Copy exported; newer patches remain unexported"
        } else {
            "Copy exported. Original input file was not changed."
        }
        .into();
        cx.notify();
    }
    pub fn show(&mut self, lens: Lens, window: &mut Window, cx: &mut Context<Self>) {
        self.lens = lens;
        self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        self.focus.focus(window);
        cx.notify();
    }
    pub fn location_for_source(&self, path: &std::path::Path, line: usize) -> Option<usize> {
        // Many instructions can belong to one source line. A representation
        // round trip must retain the exact selected instruction or byte.
        if self
            .image
            .source_at_offset(self.offset)
            .is_some_and(|location| {
                std::path::Path::new(&location.path) == path && location.line as usize == line + 1
            })
        {
            return Some(self.offset);
        }
        self.image
            .source_locations
            .iter()
            .find(|location| {
                std::path::Path::new(&location.path) == path && location.line as usize == line + 1
            })
            .and_then(|location| location.file_offset)
    }
    pub fn go(&mut self, offset: usize, cx: &mut Context<Self>) {
        if offset >= self.image.bytes().len() {
            self.message = "Location is outside file-backed bytes.".into();
            cx.notify();
            return;
        }
        if offset != self.offset {
            self.back.push(self.offset);
            if self.back.len() > 128 {
                self.back.remove(0);
            }
        }
        self.offset = offset;
        self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        self.preview = None;
        self.decode();
        cx.notify();
    }
    fn decode(&mut self) {
        match self
            .image
            .disassemble_as(&self.architecture, self.offset, 512, 64)
        {
            Ok(instructions) => {
                self.instructions = instructions;
                self.decode_error = None;
            }
            Err(error) => {
                self.instructions.clear();
                self.decode_error = Some(error);
            }
        }
    }
    fn refresh_entropy(&mut self, cx: &mut Context<Self>) {
        let image = self.image.clone();
        let revision = self.revision;
        let work = cx.background_executor().spawn(async move {
            image
                .entropy(
                    0..image.bytes().len(),
                    image.bytes().len().div_ceil(96).max(1),
                )
                .unwrap_or_default()
                .into_iter()
                .map(|bin| (bin.offset, bin.entropy))
                .collect()
        });
        cx.spawn(async move |this, cx| {
            let entropy = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.revision == revision {
                    this.entropy = entropy;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn history(&mut self, redo: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let item = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some((revision, image)) = item {
            let current = (self.revision, self.image.clone());
            if redo {
                self.undo.push(current);
            } else {
                self.redo.push(current);
            }
            self.revision = revision;
            self.image = image;
            self.preview = None;
            self.decode();
            self.refresh_entropy(cx);
            self.refresh_analysis(cx);
            self.message = "Patch history updated; location retained.".into();
            cx.notify();
        }
    }
    fn run(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.preview = None;
        let text = self.input.read(cx).text();
        if text.len() > 4096 {
            self.message = "Command is too long (maximum 4096 UTF-8 bytes).".into();
            cx.notify();
            return;
        }
        let text = text.trim();
        match self.command {
            Command::Go => {
                let (file_offset, number) = match text.strip_prefix('@') {
                    Some(s) => (true, s),
                    None => (false, text),
                };
                let parsed = u64::from_str_radix(number.trim_start_matches("0x"), 16);
                let offset = parsed.ok().and_then(|value| {
                    if file_offset {
                        usize::try_from(value).ok()
                    } else {
                        self.image.address_to_offset(value)
                    }
                });
                if let Some(offset) = offset {
                    self.go(offset, cx);
                } else {
                    self.message = "No unique file-backed location. Use a hexadecimal address or @file-offset.".into();
                }
            }
            Command::Find => {
                if text.is_empty() {
                    self.message = "Enter a symbol name or string.".into();
                } else {
                    let query = text.to_lowercase();
                    let image = self.image.clone();
                    let work = cx.background_executor().spawn(async move {
                        let mut found: Vec<_> = image
                            .symbols
                            .iter()
                            .filter(|symbol| symbol.name.to_lowercase().contains(&query))
                            .filter_map(|symbol| {
                                symbol
                                    .file_offset
                                    .map(|offset| (offset, symbol.name.clone()))
                            })
                            .take(128)
                            .collect();
                        for string in image.strings(4, 100_000) {
                            if found.len() == 128 {
                                break;
                            }
                            if string.text.to_lowercase().contains(&query) {
                                found.push((string.offset, string.text));
                            }
                        }
                        found
                    });
                    self.busy = true;
                    cx.spawn(async move |this, cx| {
                        let found = work.await;
                        let _ = this.update(cx, |this, cx| {
                            this.matches = found;
                            this.busy = false;
                            this.lens = Lens::Strings;
                            this.message = format!(
                                "{} results (up to 128; first 100,000 strings scanned)",
                                this.matches.len()
                            );
                            cx.notify();
                        });
                    })
                    .detach();
                }
            }
            Command::Patch => {
                let parsed: Result<Vec<u8>, _> = text
                    .split_whitespace()
                    .map(|s| {
                        if s.len() == 2 {
                            u8::from_str_radix(s, 16)
                        } else {
                            u8::from_str_radix("invalid", 16)
                        }
                    })
                    .collect();
                match parsed {
                    Ok(bytes)
                        if !bytes.is_empty()
                            && bytes.len() <= 256
                            && self
                                .offset
                                .checked_add(bytes.len())
                                .is_some_and(|end| end <= self.image.bytes().len()) =>
                    {
                        self.message = format!(
                            "Preview: {} byte(s) at file offset {:#x}. Apply changes only in memory.",
                            bytes.len(),
                            self.offset
                        );
                        self.preview = Some((self.offset, bytes, self.revision));
                    }
                    _ => {
                        self.message =
                            "Use 1-256 two-digit hex bytes within the file, such as 90 90.".into()
                    }
                }
            }
            Command::Assemble => {
                let bitness = match self.architecture {
                    Architecture::X86 => 32,
                    Architecture::X86_64 => 64,
                    _ => {
                        self.message =
                            "Instruction assembly currently supports x86 / x86-64.".into();
                        cx.notify();
                        return;
                    }
                };
                let Some(instruction) = self
                    .instructions
                    .first()
                    .filter(|instruction| instruction.valid)
                else {
                    self.message = "Select a decodable instruction first.".into();
                    cx.notify();
                    return;
                };
                let size = instruction.bytes.len();
                let origin = instruction.address;
                let offset = self.offset;
                let revision = self.revision;
                let input_generation = self.input_generation;
                let text = text.to_owned();
                self.busy = true;
                let work = cx
                    .background_executor()
                    .spawn(async move { systems_io::assemble_instruction(&text, bitness, origin) });
                cx.spawn(async move |this,cx| { let result = work.await; let _ = this.update(cx,|this,cx| {
                    this.busy = false;
                    match result {
                        Ok(bytes) if bytes.len() == size && this.offset == offset && this.revision == revision
                            && this.input_generation == input_generation && this.command == Command::Assemble => {
                            this.message = format!("Assembly preview: {} bytes. Apply to replace this instruction in memory.",bytes.len());
                            this.preview = Some((offset,bytes,revision));
                        }
                        Ok(bytes) => this.message = format!("Not applied: encoding is {} bytes; selected instruction is {size}. Location must stay unchanged and sizes must match.",bytes.len()),
                        Err(error) => this.message = format!("Assembly failed: {error}"),
                    }
                    cx.notify();
                }); }).detach();
            }
        }
        cx.notify();
    }
    fn apply_patch(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some((offset, bytes, revision)) = self.preview.take() else {
            return;
        };
        if revision != self.revision {
            self.message = "Stale preview; run the command again.".into();
            cx.notify();
            return;
        }
        if self
            .image
            .bytes()
            .get(offset..offset.saturating_add(bytes.len()))
            == Some(bytes.as_slice())
        {
            self.message = "Bytes already match; no patch was added.".into();
            cx.notify();
            return;
        }
        let image = self.image.clone();
        self.busy = true;
        let work = cx
            .background_executor()
            .spawn(async move { image.patched(offset, &bytes) });
        cx.spawn(async move |this,cx| { let result = work.await; let _ = this.update(cx,|this,cx| {
            this.busy = false;
            match result {
                Ok(image) => {
                    this.undo.push((this.revision,this.image.clone()));
                    while this.undo.len() > 32 || this.undo.iter().map(|(_,image)|image.bytes().len()).sum::<usize>() > 128*1024*1024 { this.undo.remove(0); }
                    this.redo.clear(); this.image = Arc::new(image); this.revision = this.next_revision; this.next_revision += 1;
                    this.decode(); this.refresh_entropy(cx); this.message = "Patch applied in memory. Export copy to write a new file; source mappings describe the original build.".into();
                }
                Err(error) => this.message = format!("Patch rejected; buffer retained: {error}"),
            }
            cx.notify();
        }); }).detach();
    }
    fn source(&mut self, cx: &mut Context<Self>) {
        if let Some(location) = self.image.source_at_offset(self.offset).cloned() {
            cx.emit(WorkbenchEvent::Source(location));
        } else {
            self.message = "No source-line mapping at this location. Stripped binaries do not contain recoverable original source.".into();
            cx.notify();
        }
    }
    fn overview(&self, cx: &Context<Self>) -> Div {
        let mut body = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(34.0))
                    .px_3()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(0xd4dce7))
                            .child(format!("{} / {}", self.image.format, self.architecture)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(0x8590a3))
                            .child(format!(
                                "{} bytes   {} sections",
                                self.image.bytes().len(),
                                self.image.sections.len()
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .h(px(24.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(0x8590a3))
                            .child("BYTE DISTRIBUTION"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(0x8590a3))
                            .child("0 — 8 bits / byte"),
                    ),
            );
        body = body.child(
            div()
                .h(px(56.0))
                .mx_3()
                .mb_3()
                .flex()
                .items_end()
                .gap(px(1.0))
                .children(
                    self.entropy
                        .iter()
                        .enumerate()
                        .map(|(index, (offset, entropy))| {
                            let offset = *offset;
                            div()
                                .id(("entropy", index))
                                .flex_1()
                                .h(px((*entropy as f32 / 8.0 * 52.0).max(2.0)))
                                .bg(rgb(if *entropy > 6.5 { 0xe0af68 } else { 0x7aa2f7 }))
                                .hover(|style| style.bg(rgb(0x7dcfff)))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.go(offset, cx);
                                    this.lens = Lens::Bytes;
                                }))
                        }),
                ),
        );
        body = body.child(
            div()
                .flex()
                .items_center()
                .h(px(24.0))
                .px_3()
                .border_t_1()
                .border_b_1()
                .border_color(rgb(0x2a313c))
                .text_size(px(11.0))
                .text_color(rgb(0x8590a3))
                .child(div().w(px(150.0)).flex_shrink_0().child("SECTION"))
                .child(div().w(px(130.0)).flex_shrink_0().child("ADDRESS"))
                .child(div().flex_1().child("FILE SIZE")),
        );
        for (index, section) in self.image.sections.iter().take(256).enumerate() {
            let offset = section.file_offset;
            let backed = section.file_size > 0;
            let selected = backed
                && self.offset >= offset
                && self.offset < offset.saturating_add(section.file_size);
            body = body.child(
                div()
                    .id(("section", index))
                    .flex()
                    .items_center()
                    .h(px(24.0))
                    .flex_shrink_0()
                    .px_3()
                    .cursor_pointer()
                    .border_l_2()
                    .border_color(rgb(if selected { 0x7aa2f7 } else { 0x101216 }))
                    .when(selected, |row| row.bg(rgb(0x1c2430)))
                    .hover(|style| style.bg(rgb(0x1c2430)))
                    .font_family("DejaVu Sans Mono")
                    .text_size(px(12.0))
                    .child(
                        div()
                            .w(px(148.0))
                            .flex_shrink_0()
                            .truncate()
                            .text_color(rgb(if section.executable {
                                0x7dcfff
                            } else {
                                0xd4dce7
                            }))
                            .child(section.name.clone()),
                    )
                    .child(
                        div()
                            .w(px(130.0))
                            .flex_shrink_0()
                            .text_color(rgb(0x8590a3))
                            .child(format!("{:x}", section.address)),
                    )
                    .child(div().flex_1().text_color(rgb(0x8590a3)).child(if backed {
                        format!("{} B", section.file_size)
                    } else {
                        "zero-fill".into()
                    }))
                    .when(section.executable, |row| {
                        row.child(
                            div()
                                .text_size(px(10.0))
                                .text_color(rgb(0x9ece6a))
                                .child("CODE"),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if backed {
                            this.go(offset, cx);
                            this.lens = Lens::Assembly;
                        } else {
                            this.message =
                                "This section is zero-fill; it has no bytes in the file.".into();
                            cx.notify();
                        }
                    })),
            );
        }
        body
    }
    fn assembly(&self, cx: &Context<Self>) -> Div {
        // DejaVu Sans Mono's advance is about 0.60 em. Reserve 0.65 em
        // per character plus spacing so zoomed text remains within its column.
        let address_digits = self
            .instructions
            .iter()
            .map(|instruction| format!("{:08x}", instruction.address).len())
            .max()
            .unwrap_or(8);
        let mnemonic_chars = self
            .instructions
            .iter()
            .map(|instruction| instruction.text.split(' ').next().unwrap_or_default().len())
            .max()
            .unwrap_or(7)
            .max(7);
        let address_width = address_digits as f32 * self.font_size * 0.65 + 12.0;
        let mnemonic_width = mnemonic_chars as f32 * self.font_size * 0.65 + 10.0;
        let row_height = (self.font_size * 1.5 + 4.0).max(23.0);
        let mut body = div().flex().flex_col().min_w_full().child(
            div()
                .flex()
                .items_center()
                .h(px(24.0))
                .pl(px(10.0))
                .pr_2()
                .border_b_1()
                .border_color(rgb(0x2a313c))
                .text_size(px(11.0))
                .text_color(rgb(0x8590a3))
                .child(div().w(px(address_width)).flex_shrink_0().child("ADDRESS"))
                .child(div().w(px(94.0)).flex_shrink_0().child("BYTES"))
                .child(div().flex_1().child("INSTRUCTION")),
        );
        if let Some(error) = &self.decode_error {
            return body.child(
                div()
                    .p_3()
                    .text_size(px(12.0))
                    .text_color(rgb(0xe0af68))
                    .whitespace_normal()
                    .child(error.clone()),
            );
        }
        for instruction in &self.instructions {
            let offset = instruction.offset;
            let target = instruction.branch_target;
            let selected = self.offset == offset;
            let (mnemonic, operands) = instruction
                .text
                .split_once(' ')
                .unwrap_or((&instruction.text, ""));
            let encoded = if instruction.bytes.len() > 4 {
                format!("{} …", hex(&instruction.bytes[..4]))
            } else {
                hex(&instruction.bytes)
            };
            body = body.child(div().id(("instruction-row", offset)).flex().items_center()
                .h(px(row_height)).flex_shrink_0().border_l_2()
                .border_color(rgb(if selected { 0x7aa2f7 } else { 0x101216 }))
                .when(selected, |row| row.bg(rgb(0x1c2430)))
                .child(div().id("select").flex().items_center().flex_1().min_w_0().h_full().px_2()
                    .cursor_pointer().hover(|style| style.bg(rgb(0x1c2430)))
                    .font_family("DejaVu Sans Mono").text_size(px(self.font_size))
                    .child(div().w(px(address_width)).flex_shrink_0().text_color(rgb(0x8590a3))
                        .child(format!("{:08x}", instruction.address)))
                    .child(div().w(px(94.0)).flex_shrink_0().text_size(px(11.0)).text_color(rgb(0x59677c)).child(encoded))
                    .child(div().w(px(mnemonic_width)).flex_shrink_0()
                        .text_color(rgb(if instruction.valid { 0x7dcfff } else { 0xe0af68 })).child(mnemonic.to_string()))
                    .child(div().flex_1().text_color(rgb(0xd4dce7)).child(operands.to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| this.go(offset, cx))))
                .when_some(target, |row, target| row.child(button("↗").id("follow")
                    .h(px(row_height)).w(px(24.0)).text_color(rgb(0x7aa2f7))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(offset) = this.image.address_to_offset(target) { this.go(offset, cx); }
                        else { this.message = format!("Branch target {target:#x} has no unique file-backed mapping."); cx.notify(); }
                    })))));
        }
        body
    }
    fn bytes(&self, cx: &Context<Self>) -> Div {
        let start = self.offset / self.row_bytes * self.row_bytes;
        let end = (start + self.row_bytes * 64).min(self.image.bytes().len());
        let cell_width = (self.font_size * 1.55 + 3.0).max(22.0);
        let offset_digits = format!("{:08x}", end.saturating_sub(1)).len();
        let offset_width = (offset_digits as f32 * self.font_size * 0.65 + 12.0).max(84.0);
        let row_height = (self.font_size * 1.5 + 2.0).max(22.0);
        let mut header = div()
            .flex()
            .items_center()
            .h(px(24.0))
            .px_2()
            .border_b_1()
            .border_color(rgb(0x2a313c))
            .text_size(px(11.0))
            .text_color(rgb(0x59677c))
            .font_family("DejaVu Sans Mono")
            .child(div().w(px(offset_width)).flex_shrink_0().child("OFFSET"));
        for index in 0..self.row_bytes {
            header = header.child(
                div()
                    .w(px(cell_width))
                    .flex_shrink_0()
                    .when(index == 8, |cell| cell.ml(px(8.0)))
                    .child(format!("{index:02x}")),
            );
        }
        header = header.child(div().pl_3().child("TEXT"));
        let mut body = div().flex().flex_col().min_w_full().child(header);
        for (index, chunk) in self.image.bytes()[start..end]
            .chunks(self.row_bytes)
            .enumerate()
        {
            let offset = start + index * self.row_bytes;
            let mut row = div()
                .id(("hex-row", offset))
                .flex()
                .items_center()
                .h(px(row_height))
                .flex_shrink_0()
                .px_2()
                .font_family("DejaVu Sans Mono")
                .text_size(px(self.font_size))
                .child(
                    div()
                        .w(px(offset_width))
                        .flex_shrink_0()
                        .text_color(rgb(0x8590a3))
                        .child(format!("{offset:08x}")),
                );
            for (index, byte) in chunk.iter().enumerate() {
                let selected = offset + index == self.offset;
                row = row.child(
                    div()
                        .id(("byte", index))
                        .w(px(cell_width))
                        .h_full()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .when(index == 8, |cell| cell.ml(px(8.0)))
                        .when(selected, |cell| {
                            cell.bg(rgb(0x30436a)).text_color(rgb(0xd4dce7))
                        })
                        .when(!selected, |cell| {
                            cell.text_color(rgb(if *byte == 0 { 0x59677c } else { 0x7dcfff }))
                        })
                        .hover(|style| style.bg(rgb(0x1c2430)))
                        .child(format!("{byte:02x}"))
                        .on_click(cx.listener(move |this, _, _, cx| this.go(offset + index, cx))),
                );
            }
            if chunk.len() < self.row_bytes {
                row = row.child(
                    div()
                        .w(px(cell_width * (self.row_bytes - chunk.len()) as f32
                            + if chunk.len() <= 8 && self.row_bytes > 8 {
                                8.0
                            } else {
                                0.0
                            }))
                        .flex_shrink_0(),
                );
            }
            row = row.child(
                div().pl_3().text_color(rgb(0x59677c)).child(
                    chunk
                        .iter()
                        .map(|byte| {
                            if byte.is_ascii_graphic() || *byte == b' ' {
                                *byte as char
                            } else {
                                '.'
                            }
                        })
                        .collect::<String>(),
                ),
            );
            body = body.child(row);
        }
        body
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}
impl Render for Workbench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let address = self
            .image
            .offset_to_address(self.offset)
            .map(|address| format!("{address:x}"))
            .unwrap_or_else(|| "unmapped".into());
        let source = self.image.source_at_offset(self.offset).map(|source| {
            let name = std::path::Path::new(&source.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            format!("{name}:{}", source.line)
        });
        let body = match self.lens {
            Lens::Overview => self.overview(cx),
            Lens::Assembly => self.assembly(cx),
            Lens::Flow => self.flow_view(cx),
            Lens::Bytes => self.bytes(cx),
            Lens::Strings => div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(28.0))
                        .px_3()
                        .flex()
                        .items_center()
                        .text_size(px(11.0))
                        .text_color(rgb(0x8590a3))
                        .border_b_1()
                        .border_color(rgb(0x2a313c))
                        .child(format!(
                            "{} MATCHES  ·  symbols and ASCII strings",
                            self.matches.len()
                        )),
                )
                .when(self.matches.is_empty(), |body| {
                    body.child(
                        div()
                            .p_3()
                            .text_size(px(12.0))
                            .text_color(rgb(0x8590a3))
                            .child("Use Find below to search this image."),
                    )
                })
                .children(
                    self.matches
                        .iter()
                        .enumerate()
                        .map(|(index, (offset, label))| {
                            let offset = *offset;
                            div()
                                .id(("match", index))
                                .flex()
                                .items_center()
                                .h(px(24.0))
                                .px_3()
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x1c2430)))
                                .font_family("DejaVu Sans Mono")
                                .text_size(px(12.0))
                                .child(
                                    div()
                                        .w(px(92.0))
                                        .flex_shrink_0()
                                        .text_color(rgb(0x8590a3))
                                        .child(format!("{offset:08x}")),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .text_color(rgb(0x9ece6a))
                                        .child(label.clone()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.go(offset, cx);
                                    this.lens = Lens::Bytes;
                                }))
                        }),
                ),
        };
        div().track_focus(&self.focus).size_full().flex().flex_col().min_h_0().font_family("Inter")
            .bg(rgb(0x101216)).text_size(px(12.0)).text_color(rgb(0xd4dce7))
            .child(div().flex().items_center().h(px(30.0)).flex_shrink_0().px_2()
                .bg(rgb(0x15191f)).border_b_1().border_color(rgb(0x2a313c))
                .children([Lens::Overview, Lens::Assembly, Lens::Bytes, Lens::Strings].map(|lens| {
                    button(lens.name()).h(px(30.0)).px_3().border_b_2()
                        .border_color(rgb(if lens == self.lens { 0x7aa2f7 } else { 0x15191f }))
                        .when(lens == self.lens, |tab| tab.text_color(rgb(0xd4dce7)))
                        .on_click(cx.listener(move |this, _, window, cx| this.show(lens, window, cx)))
                }))
                .child(button("Source").on_click(cx.listener(|this, _, _, cx| this.source(cx))))
                .child(div().flex_1())
                .child(button("Back").on_click(cx.listener(|this, _, _, cx| {
                    if let Some(offset) = this.back.pop() { this.offset = offset; this.preview = None;
                        this.scroll.set_offset(gpui::point(px(0.0), px(0.0))); this.decode(); cx.notify(); }
                })))
                .child(button("Next").on_click(cx.listener(|this, _, _, cx| {
                    let next = if this.lens == Lens::Assembly { this.instructions.last().map(|i| i.offset + i.bytes.len()).unwrap_or(this.offset) }
                        else { this.offset.saturating_add(this.row_bytes * 32) };
                    this.go(next, cx);
                })))
                .child(button("Details").when(self.details_visible, |button| button.text_color(rgb(0x7aa2f7)))
                    .on_click(cx.listener(|this, _, _, cx| { this.details_visible = !this.details_visible; cx.notify(); }))))
            .child(div().flex().items_center().gap_3().h(px(24.0)).flex_shrink_0().px_3()
                .border_b_1().border_color(rgb(0x2a313c)).text_size(px(11.0)).text_color(rgb(0x8590a3))
                .child(format!("{} / {}", self.image.format, self.architecture))
                .child(div().font_family("DejaVu Sans Mono").text_color(rgb(0x7aa2f7)).child(address))
                .child(div().font_family("DejaVu Sans Mono").child(format!("@{:x}", self.offset)))
                .when_some(source, |line, source| line.child(div().min_w_0().truncate().child(source)))
                .child(div().flex_1())
                .when(self.is_dirty(), |line| line.child(div().text_color(rgb(0xe0af68)).child("MODIFIED"))))
            .when(self.details_visible, |root| root.child(div().id("binary-details").max_h(px(112.0)).overflow_y_scroll()
                .flex_shrink_0().bg(rgb(0x15191f)).border_b_1().border_color(rgb(0x2a313c)).px_3().py_2()
                .text_size(px(11.0)).text_color(rgb(0x8590a3))
                .child(div().whitespace_normal().child(self.path.display().to_string()))
                .child(div().flex().items_center().gap_1().py_1()
                    .child(button(if self.inspector { "Hide symbols" } else { "Show symbols" }).on_click(cx.listener(|this, _, _, cx| { this.inspector = !this.inspector; cx.notify(); })))
                    .child(button("A−").on_click(cx.listener(|this, _, _, cx| { this.font_size = (this.font_size - 1.0).max(10.0); cx.notify(); })))
                    .child(button("A+").on_click(cx.listener(|this, _, _, cx| { this.font_size = (this.font_size + 1.0).min(20.0); cx.notify(); })))
                    .child(button(format!("{} bytes / row", self.row_bytes)).on_click(cx.listener(|this, _, _, cx| { this.row_bytes = if this.row_bytes == 16 { 8 } else { 16 }; cx.notify(); })))
                    .when(matches!(self.image.architecture, Architecture::Unknown), |tools| tools.child(button("Interpret x86-64").text_color(rgb(0x7aa2f7))
                        .on_click(cx.listener(|this, _, _, cx| { this.architecture = Architecture::X86_64; this.decode(); this.lens = Lens::Assembly;
                            this.message = "Raw bytes interpreted as x86-64, base address 0. This is an explicit assumption.".into(); cx.notify(); })))))
                .child(div().whitespace_normal().child(self.command.help()))
                .child(div().whitespace_normal().child("Linear decoding; source mappings describe the original build. Entropy is a heuristic, not proof of encryption."))
                .children(self.image.warnings.iter().map(|warning| div().whitespace_normal().text_color(rgb(0xe0af68)).child(warning.clone())))
                .child(div().whitespace_normal().child(self.message.clone()))))
            .child(div().flex().flex_1().min_h_0()
                .when(self.inspector, |row| row.child(div().id("binary-symbols").w(px(168.0)).flex_shrink_0()
                    .flex().flex_col().bg(rgb(0x15191f)).border_r_1().border_color(rgb(0x2a313c))
                    .child(div().h(px(26.0)).flex_shrink_0().flex().items_center().justify_between().px_3()
                        .text_size(px(11.0)).text_color(rgb(0x8590a3)).child("SYMBOLS")
                        .child(format!("{}", self.image.symbols.len())))
                    .child(div().id("symbol-list").flex_1().min_h_0().overflow_y_scroll()
                        .children(self.image.symbols.iter().take(256)
                            .filter_map(|symbol| symbol.file_offset.map(|offset| (offset, symbol.name.clone(), symbol.size)))
                            .enumerate().map(|(index, (offset, name, size))| {
                                let selected = self.offset >= offset && self.offset < offset.saturating_add(size.max(1) as usize);
                                div().id(("symbol", index)).h(px(22.0)).flex().items_center().px_3()
                                    .cursor_pointer().hover(|style| style.bg(rgb(0x1c2430)))
                                    .when(selected, |row| row.bg(rgb(0x1c2430)).text_color(rgb(0x7aa2f7)))
                                    .when(!selected, |row| row.text_color(rgb(0x8590a3)))
                                    .text_size(px(11.5)).truncate().child(name)
                                    .on_click(cx.listener(move |this, _, _, cx| { this.go(offset, cx); this.lens = Lens::Assembly; }))
                            })))))
                .child(div().id("binary-body").flex_1().min_w_0().overflow_scroll().track_scroll(&self.scroll).child(body)))
            .child(div().flex().items_center().h(px(28.0)).flex_shrink_0().px_2()
                .border_t_1().border_color(rgb(0x2a313c)).bg(rgb(0x15191f))
                .children([Command::Go, Command::Find, Command::Assemble, Command::Patch].map(|command| {
                    button(command.name()).h(px(28.0)).border_b_2()
                        .border_color(rgb(if command == self.command { 0x7aa2f7 } else { 0x15191f }))
                        .when(command == self.command, |tab| tab.text_color(rgb(0xd4dce7)))
                        .on_click(cx.listener(move |this, _, window, cx| { this.command = command; this.preview = None;
                            this.input.read(cx).focus_handle(cx).focus(window); cx.notify(); }))
                }))
                .child(div().flex_1())
                .child(button("Undo").on_click(cx.listener(|this, _, _, cx| this.history(false, cx))))
                .child(button("Redo").on_click(cx.listener(|this, _, _, cx| this.history(true, cx))))
                .child(button("Export copy").text_color(rgb(0x7aa2f7)).on_click(cx.listener(|_, _, _, cx| cx.emit(WorkbenchEvent::Export)))))
            .child(div().flex().items_center().h(px(30.0)).flex_shrink_0().px_3().gap_2().bg(rgb(0x15191f))
                .child(div().text_color(rgb(0x7aa2f7)).child("›"))
                .child(div().flex_1().min_w_0().h(px(25.0)).child(self.input.clone()))
                .child(button(if self.busy { "Working…" } else { "Run" }).text_color(rgb(0x7aa2f7))
                    .on_click(cx.listener(|this, _, _, cx| this.run(cx)))))
            .when_some(self.preview.as_ref().map(|(_, bytes, _)| hex(bytes)), |root, bytes| root.child(
                div().flex().items_center().gap_2().h(px(30.0)).flex_shrink_0().px_3().bg(rgb(0x1c2a3d))
                    .child(div().text_size(px(11.0)).text_color(rgb(0x8590a3)).child("PREVIEW"))
                    .child(div().flex_1().min_w_0().truncate().font_family("DejaVu Sans Mono").text_size(px(12.0)).text_color(rgb(0x7dcfff)).child(bytes.clone()))
                    .child(button("Copy bytes").on_click(cx.listener(move |_, _, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(bytes.clone())))))
                    .child(button("Apply patch").bg(rgb(0x7aa2f7)).text_color(rgb(0x101216))
                        .on_click(cx.listener(|this, _, _, cx| this.apply_patch(cx))))))
            .child(div().id("systems-message").h(px(22.0)).flex_shrink_0().flex().items_center().px_3()
                .border_t_1().border_color(rgb(0x2a313c)).text_size(px(11.0)).text_color(rgb(0x8590a3))
                .child(div().truncate().child(self.message.clone())))
    }
}
