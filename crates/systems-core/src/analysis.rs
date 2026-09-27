//! Bounded recursive traversal of declared executable regions. Function roots
//! and membership are candidates inferred from metadata and direct calls; this
//! does not prove code/data separation, recover indirect targets, or decompile.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::atomic::{AtomicBool, Ordering},
};

use iced_x86::{Decoder, DecoderOptions, FlowControl, Formatter, IntelFormatter, OpKind};

use crate::{Architecture, BinaryFormat, BinaryImage, Instruction};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalysisLimits {
    pub max_functions: usize,
    pub max_blocks: usize,
    pub max_instructions: usize,
    pub max_bytes: usize,
}

impl Default for AnalysisLimits {
    fn default() -> Self {
        Self {
            max_functions: 128,
            max_blocks: 4096,
            max_instructions: 100_000,
            max_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub functions: Vec<AnalyzedFunction>,
    pub references: Vec<CrossReference>,
    pub warnings: Vec<String>,
    /// True means a configured limit omitted reachable candidates or blocks.
    pub truncated: bool,
    /// Instruction rows across returned functions; shared tails count per function.
    pub instruction_count: usize,
}

#[derive(Clone, Debug)]
pub struct AnalyzedFunction {
    pub name: String,
    pub address: u64,
    pub offset: usize,
    pub provenance: FunctionProvenance,
    pub blocks: Vec<BasicBlock>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunctionProvenance {
    EntryPoint,
    ExecutableSymbol,
    DirectCall,
}

#[derive(Clone, Debug)]
pub struct BasicBlock {
    pub start_address: u64,
    pub start_offset: usize,
    pub instructions: Vec<Instruction>,
    pub edges: Vec<FlowEdge>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Fallthrough,
    ConditionalBranch,
    Jump,
    Call,
    IndirectCall,
    IndirectJump,
    Return,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeResolution {
    Resolved,
    Unmapped,
    NonExecutable,
    Indirect,
    Terminal,
    Overlap,
    Limit,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowEdge {
    pub from_address: u64,
    pub from_offset: usize,
    pub target_address: Option<u64>,
    pub target_offset: Option<usize>,
    pub kind: EdgeKind,
    pub resolution: EdgeResolution,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    Call,
    ConditionalBranch,
    Jump,
    IndirectCall,
    IndirectJump,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossReference {
    pub from_address: u64,
    pub from_offset: usize,
    pub target_address: Option<u64>,
    pub target_offset: Option<usize>,
    pub kind: ReferenceKind,
    pub resolution: EdgeResolution,
}

#[derive(Clone)]
struct Node {
    instruction: Instruction,
    edges: Vec<FlowEdge>,
    ends_block: bool,
}

#[derive(Clone)]
struct Seed {
    name: String,
    address: u64,
    offset: usize,
    provenance: FunctionProvenance,
}

struct Region {
    address: u64,
    offset: usize,
    size: usize,
    executable: bool,
}
struct RangeIndex {
    entries: Vec<(u64, u64, usize)>,
    max_ends: Vec<u64>,
}

impl RangeIndex {
    fn new(mut entries: Vec<(u64, u64, usize)>) -> Self {
        entries.sort_unstable();
        let mut largest = 0;
        let max_ends = entries
            .iter()
            .map(|(_, end, _)| {
                largest = largest.max(*end);
                largest
            })
            .collect();
        Self { entries, max_ends }
    }

    fn find(&self, value: u64) -> Result<usize, EdgeResolution> {
        let end = self
            .entries
            .partition_point(|(start, _, _)| *start <= value);
        let mut result = None;
        for index in (0..end).rev() {
            if self.max_ends[index] <= value {
                break;
            }
            let (_, end, region) = self.entries[index];
            if value < end {
                if result.is_some() {
                    return Err(EdgeResolution::Overlap);
                }
                result = Some(region);
            }
        }
        result.ok_or(EdgeResolution::Unmapped)
    }
}

struct AddressSpace {
    regions: Vec<Region>,
    addresses: RangeIndex,
    offsets: RangeIndex,
}

impl AddressSpace {
    fn new(image: &BinaryImage) -> Self {
        let mut executable = BTreeSet::new();
        for section in &image.sections {
            if section.executable {
                executable.insert((
                    section.address,
                    section.file_offset,
                    section.size.min(section.file_size as u64) as usize,
                ));
            }
        }
        let regions: Vec<_> = image
            .mappings
            .iter()
            .map(|mapping| Region {
                address: mapping.address,
                offset: mapping.offset,
                size: mapping.size,
                executable: executable.contains(&(mapping.address, mapping.offset, mapping.size)),
            })
            .collect();
        let addresses = RangeIndex::new(
            regions
                .iter()
                .enumerate()
                .map(|(index, region)| (region.address, region.address + region.size as u64, index))
                .collect(),
        );
        let offsets = RangeIndex::new(
            regions
                .iter()
                .enumerate()
                .map(|(index, region)| {
                    (
                        region.offset as u64,
                        (region.offset + region.size) as u64,
                        index,
                    )
                })
                .collect(),
        );
        Self {
            regions,
            addresses,
            offsets,
        }
    }

    fn locate(&self, address: u64) -> Result<(usize, usize), EdgeResolution> {
        let index = self.addresses.find(address)?;
        let region = &self.regions[index];
        let offset = region.offset + (address - region.address) as usize;
        if self.offsets.find(offset as u64)? != index {
            return Err(EdgeResolution::Overlap);
        }
        if !region.executable {
            return Err(EdgeResolution::NonExecutable);
        }
        Ok((offset, index))
    }

    fn window(&self, address: u64) -> Result<(usize, usize), EdgeResolution> {
        let (offset, index) = self.locate(address)?;
        let mut length = 1;
        while length < 15 {
            let Some(next) = address.checked_add(length as u64) else {
                break;
            };
            if self.locate(next) != Ok((offset + length, index)) {
                break;
            }
            length += 1;
        }
        Ok((offset, length))
    }
}

impl BinaryImage {
    pub fn analyze(
        &self,
        architecture: &Architecture,
        limits: AnalysisLimits,
        cancelled: &AtomicBool,
    ) -> Result<Analysis, String> {
        check_cancelled(cancelled)?;
        validate_limits(limits)?;
        let bitness = match architecture {
            Architecture::X86 => 32,
            Architecture::X86_64 => 64,
            _ => {
                return Err(format!(
                    "Control-flow analysis supports x86 and x86-64, not {architecture}"
                ));
            }
        };
        if self.format == BinaryFormat::Raw {
            return Err("Raw bytes have no declared executable regions; use explicit linear disassembly instead".into());
        }
        if architecture != &self.architecture {
            return Err("Analysis architecture must match the binary metadata".into());
        }
        let space = AddressSpace::new(self);
        let mut analyzer = Analyzer {
            image: self,
            space,
            bitness,
            limits,
            cancelled,
            seeds: BTreeMap::new(),
            pending_functions: VecDeque::new(),
            cache: BTreeMap::new(),
            rejected: BTreeMap::new(),
            included_instructions: 0,
            included_bytes: 0,
            blocks_remaining: limits.max_blocks,
            result: Analysis {
                functions: Vec::new(),
                references: Vec::new(),
                warnings: Vec::new(),
                truncated: false,
                instruction_count: 0,
            },
        };
        if let Some(address) = self.entry {
            let name = self
                .symbols
                .iter()
                .find(|symbol| symbol.address == address)
                .map(|symbol| symbol.name.clone())
                .unwrap_or_else(|| "entry".into());
            analyzer.seed(address, name, FunctionProvenance::EntryPoint);
        }
        for symbol in &self.symbols {
            check_cancelled(cancelled)?;
            analyzer.seed(
                symbol.address,
                symbol.name.clone(),
                FunctionProvenance::ExecutableSymbol,
            );
        }
        if analyzer.seeds.is_empty() {
            analyzer.warn("No unambiguous executable entry point or symbol is available; arbitrary bytes were not decoded".into());
        }
        while let Some(offset) = analyzer.pending_functions.pop_front() {
            check_cancelled(cancelled)?;
            if analyzer.exhausted() {
                analyzer.limit();
                break;
            }
            let seed = analyzer.seeds[&offset].clone();
            let function = analyzer.function(seed)?;
            analyzer.result.functions.push(function);
        }
        analyzer.finish()
    }
}

fn validate_limits(limits: AnalysisLimits) -> Result<(), String> {
    if limits.max_functions == 0
        || limits.max_functions > 4096
        || limits.max_blocks == 0
        || limits.max_blocks > 65_536
        || limits.max_instructions == 0
        || limits.max_instructions > 1_000_000
        || limits.max_bytes == 0
        || limits.max_bytes > 64 * 1024 * 1024
    {
        return Err("Analysis limits must be nonzero and at most 4,096 functions, 65,536 blocks, 1,000,000 instruction rows and 64 MiB of decoded bytes".into());
    }
    Ok(())
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("Control-flow analysis cancelled".into())
    } else {
        Ok(())
    }
}

struct Analyzer<'a> {
    image: &'a BinaryImage,
    space: AddressSpace,
    bitness: u32,
    limits: AnalysisLimits,
    cancelled: &'a AtomicBool,
    seeds: BTreeMap<usize, Seed>,
    pending_functions: VecDeque<usize>,
    cache: BTreeMap<usize, Node>,
    rejected: BTreeMap<usize, EdgeResolution>,
    included_instructions: usize,
    included_bytes: usize,
    blocks_remaining: usize,
    result: Analysis,
}

impl Analyzer<'_> {
    fn warn(&mut self, warning: String) {
        if self.result.warnings.len() < 64 && !self.result.warnings.contains(&warning) {
            self.result.warnings.push(warning);
        }
    }

    fn limit(&mut self) {
        self.result.truncated = true;
        self.warn("Analysis stopped at a configured limit; unvisited edges and omitted functions are not recovered code".into());
    }

    fn exhausted(&self) -> bool {
        self.included_instructions >= self.limits.max_instructions
            || self.included_bytes >= self.limits.max_bytes
            || self.blocks_remaining == 0
    }

    fn seed(&mut self, address: u64, name: String, provenance: FunctionProvenance) -> bool {
        let Ok((offset, _)) = self.space.locate(address) else {
            return false;
        };
        if self.seeds.contains_key(&offset) {
            return true;
        }
        if self.seeds.len() == self.limits.max_functions {
            self.limit();
            return false;
        }
        self.seeds.insert(
            offset,
            Seed {
                name,
                address,
                offset,
                provenance,
            },
        );
        self.pending_functions.push_back(offset);
        true
    }

    fn edge(&self, instruction: &Instruction, kind: EdgeKind, target: Option<u64>) -> FlowEdge {
        let (target_offset, resolution) = if let Some(address) = target {
            match self.space.locate(address) {
                Ok((offset, _)) => (Some(offset), EdgeResolution::Resolved),
                Err(error) => (self.image.address_to_offset(address), error),
            }
        } else {
            (
                None,
                if matches!(kind, EdgeKind::Return | EdgeKind::Stop) {
                    EdgeResolution::Terminal
                } else {
                    EdgeResolution::Indirect
                },
            )
        };
        FlowEdge {
            from_address: instruction.address,
            from_offset: instruction.offset,
            target_address: target,
            target_offset,
            kind,
            resolution,
        }
    }

    fn decode(&mut self, address: u64) -> Result<Node, EdgeResolution> {
        let (offset, available) = self.space.window(address)?;
        if let Some(node) = self.cache.get(&offset) {
            return Ok(node.clone());
        }
        if let Some((&start, previous)) = self.cache.range(..offset).next_back()
            && start + previous.instruction.bytes.len() > offset
        {
            self.rejected.insert(offset, EdgeResolution::Overlap);
            return Err(EdgeResolution::Overlap);
        }
        let bytes = &self.image.bytes()[offset..offset + available];
        let mut decoder = Decoder::with_ip(self.bitness, bytes, address, DecoderOptions::NONE);
        let decoded = decoder.decode();
        let valid = !decoded.is_invalid();
        let length = if valid { decoded.len() } else { 1 };
        if self
            .cache
            .range(offset + 1..offset + length)
            .next()
            .is_some()
        {
            self.rejected.insert(offset, EdgeResolution::Overlap);
            return Err(EdgeResolution::Overlap);
        }
        let near_target = (0..decoded.op_count())
            .any(|index| {
                matches!(
                    decoded.op_kind(index),
                    OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64
                )
            })
            .then(|| decoded.near_branch_target());
        let mut text = String::new();
        if valid {
            let mut formatter = IntelFormatter::new();
            formatter.options_mut().set_hex_prefix("0x");
            formatter.options_mut().set_hex_suffix("");
            formatter.options_mut().set_uppercase_hex(false);
            formatter.format(&decoded, &mut text);
        } else {
            text = format!("db 0x{:02x} ; invalid or truncated instruction", bytes[0]);
        }
        let instruction = Instruction {
            offset,
            address,
            bytes: bytes[..length].to_vec(),
            text,
            branch_target: near_target,
            valid,
        };
        let next = address.checked_add(length as u64);
        let flow = decoded.flow_control();
        let mut edges = if !valid {
            let mut edge = self.edge(&instruction, EdgeKind::Stop, None);
            edge.resolution = EdgeResolution::Invalid;
            vec![edge]
        } else {
            match flow {
                FlowControl::Next => vec![self.edge(&instruction, EdgeKind::Fallthrough, next)],
                FlowControl::ConditionalBranch | FlowControl::XbeginXabortXend => vec![
                    self.edge(&instruction, EdgeKind::ConditionalBranch, near_target),
                    self.edge(&instruction, EdgeKind::Fallthrough, next),
                ],
                FlowControl::UnconditionalBranch => vec![self.edge(
                    &instruction,
                    if near_target.is_some() {
                        EdgeKind::Jump
                    } else {
                        EdgeKind::IndirectJump
                    },
                    near_target,
                )],
                FlowControl::Call | FlowControl::IndirectCall => vec![
                    self.edge(
                        &instruction,
                        if near_target.is_some() {
                            EdgeKind::Call
                        } else {
                            EdgeKind::IndirectCall
                        },
                        near_target,
                    ),
                    self.edge(&instruction, EdgeKind::Fallthrough, next),
                ],
                FlowControl::IndirectBranch => {
                    vec![self.edge(&instruction, EdgeKind::IndirectJump, None)]
                }
                FlowControl::Return => vec![self.edge(&instruction, EdgeKind::Return, None)],
                FlowControl::Interrupt | FlowControl::Exception => {
                    vec![self.edge(&instruction, EdgeKind::Stop, None)]
                }
            }
        };
        if next.is_none() {
            for edge in &mut edges {
                if edge.kind == EdgeKind::Fallthrough {
                    edge.resolution = EdgeResolution::Unmapped;
                }
            }
        }
        let node = Node {
            instruction,
            edges,
            ends_block: !valid || flow != FlowControl::Next,
        };
        self.cache.insert(offset, node.clone());
        Ok(node)
    }

    fn function(&mut self, seed: Seed) -> Result<AnalyzedFunction, String> {
        let mut pending = VecDeque::from([seed.address]);
        let mut queued = BTreeSet::from([seed.address]);
        let mut nodes = BTreeMap::new();
        let mut leaders = BTreeSet::from([seed.offset]);
        while let Some(address) = pending.pop_front() {
            check_cancelled(self.cancelled)?;
            if self.exhausted() {
                self.limit();
                break;
            }
            let node = match self.decode(address) {
                Ok(node) => node,
                Err(error) => {
                    self.warn(format!("Traversal at {address:#x} stopped: {error:?}"));
                    continue;
                }
            };
            let offset = node.instruction.offset;
            if nodes.contains_key(&offset) {
                continue;
            }
            if self.included_bytes + node.instruction.bytes.len() > self.limits.max_bytes {
                self.limit();
                break;
            }
            self.included_instructions += 1;
            self.included_bytes += node.instruction.bytes.len();
            for edge in &node.edges {
                if edge.resolution != EdgeResolution::Resolved {
                    continue;
                }
                let target = edge.target_address.expect("resolved target address");
                let target_offset = edge.target_offset.expect("resolved target offset");
                if edge.kind == EdgeKind::Call {
                    self.seed(
                        target,
                        format!("sub_{target:x}"),
                        FunctionProvenance::DirectCall,
                    );
                } else {
                    if node.ends_block {
                        leaders.insert(target_offset);
                    }
                    if target_offset != seed.offset && self.seeds.contains_key(&target_offset) {
                        continue;
                    }
                    if queued.insert(target) {
                        pending.push_back(target);
                    }
                }
            }
            nodes.insert(offset, node);
        }
        // Every reachable instruction not preceded by a straight-line predecessor
        // is a leader; branch targets split an earlier decoded linear run.
        for (&offset, node) in &nodes {
            if node.ends_block {
                continue;
            }
            if let Some(edge) = node.edges.first()
                && edge.resolution == EdgeResolution::Resolved
                && let Some(target) = edge.target_offset
                && (!nodes.contains_key(&target) || target != offset + node.instruction.bytes.len())
            {
                leaders.insert(target);
            }
        }
        let mut blocks = Vec::new();
        let mut assigned = BTreeSet::new();
        // `nodes` order also covers roots discovered through shared-tail paths.
        let starts: Vec<_> = leaders.into_iter().chain(nodes.keys().copied()).collect();
        for start in starts {
            check_cancelled(self.cancelled)?;
            if assigned.contains(&start) || !nodes.contains_key(&start) {
                continue;
            }
            if self.blocks_remaining == 0 {
                self.limit();
                break;
            }
            self.blocks_remaining -= 1;
            let mut instructions = Vec::new();
            let mut offset = start;
            let (address, edges) = loop {
                let node = &nodes[&offset];
                instructions.push(node.instruction.clone());
                assigned.insert(offset);
                let next = node.edges.first().and_then(|edge| edge.target_offset);
                let split = next.is_none_or(|next| {
                    next <= offset
                        || !nodes.contains_key(&next)
                        || assigned.contains(&next)
                        || queued.contains(&nodes[&next].instruction.address) && node.ends_block
                });
                let other_leader = next.is_some_and(|next| {
                    // A target with more than its sequential predecessor starts a block.
                    nodes.values().any(|candidate| {
                        candidate.ends_block
                            && candidate.edges.iter().any(|edge| {
                                edge.kind != EdgeKind::Call && edge.target_offset == Some(next)
                            })
                    })
                });
                if node.ends_block || split || other_leader {
                    break (nodes[&start].instruction.address, node.edges.clone());
                }
                offset = next.expect("straight-line successor");
            };
            blocks.push(BasicBlock {
                start_address: address,
                start_offset: start,
                instructions,
                edges,
            });
        }
        blocks.sort_by_key(|block| block.start_address);
        Ok(AnalyzedFunction {
            name: seed.name,
            address: seed.address,
            offset: seed.offset,
            provenance: seed.provenance,
            blocks,
        })
    }

    fn finish(mut self) -> Result<Analysis, String> {
        check_cancelled(self.cancelled)?;
        let included: BTreeSet<_> = self
            .result
            .functions
            .iter()
            .flat_map(|function| function.blocks.iter())
            .flat_map(|block| {
                block
                    .instructions
                    .iter()
                    .map(|instruction| instruction.offset)
            })
            .collect();
        for function in &mut self.result.functions {
            for block in &mut function.blocks {
                self.result.instruction_count += block.instructions.len();
                for edge in &mut block.edges {
                    if edge.resolution == EdgeResolution::Resolved
                        && let Some(offset) = edge.target_offset
                    {
                        if let Some(reason) = self.rejected.get(&offset) {
                            edge.resolution = *reason;
                        } else if !included.contains(&offset) {
                            edge.resolution = EdgeResolution::Limit;
                        }
                    }
                }
            }
        }
        for function in &self.result.functions {
            for block in &function.blocks {
                for edge in &block.edges {
                    let kind = match edge.kind {
                        EdgeKind::Call => ReferenceKind::Call,
                        EdgeKind::ConditionalBranch => ReferenceKind::ConditionalBranch,
                        EdgeKind::Jump => ReferenceKind::Jump,
                        EdgeKind::IndirectCall => ReferenceKind::IndirectCall,
                        EdgeKind::IndirectJump => ReferenceKind::IndirectJump,
                        _ => continue,
                    };
                    let reference = CrossReference {
                        from_address: edge.from_address,
                        from_offset: edge.from_offset,
                        target_address: edge.target_address,
                        target_offset: edge.target_offset,
                        kind,
                        resolution: edge.resolution,
                    };
                    if !self.result.references.contains(&reference) {
                        self.result.references.push(reference);
                    }
                }
            }
        }
        Ok(self.result)
    }
}
