//! Pure, bounded binary inspection. No filesystem, process execution, or GPUI.
//!
//! Addresses are virtual addresses for recognized images, file offsets for raw
//! data. A mapping is absent when it is ambiguous or has no file-backed bytes.
//! Disassembly is a linear interpretation, not proof that bytes are reachable
//! instructions. DWARF is optional and never causes source files to be read.

use std::{fmt, ops::Range, sync::Arc};

use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter, OpKind};
use object::{Object, ObjectSection, ObjectSymbol};

mod analysis;
mod dwarf;
pub use analysis::{
    Analysis, AnalysisLimits, AnalyzedFunction, BasicBlock, CrossReference, EdgeKind,
    EdgeResolution, FlowEdge, FunctionProvenance, ReferenceKind,
};

const MAX_IMAGE_BYTES: usize = 512 * 1024 * 1024;
const MAX_SECTIONS: usize = 16_384;
const MAX_SYMBOLS: usize = 200_000;
const MAX_NAME_BYTES: usize = 4096;
const MAX_METADATA_TEXT: usize = 16 * 1024 * 1024;
const MAX_DECODE_BYTES: usize = 1024 * 1024;
const MAX_INSTRUCTIONS: usize = 16_384;
const MAX_ENTROPY_BINS: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryFormat {
    Elf,
    Pe,
    MachO,
    Raw,
}

impl fmt::Display for BinaryFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Elf => "ELF",
            Self::Pe => "PE",
            Self::MachO => "Mach-O",
            Self::Raw => "Raw bytes",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Architecture {
    X86,
    X86_64,
    Other(String),
    Unknown,
}

impl fmt::Display for Architecture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::X86 => f.write_str("x86"),
            Self::X86_64 => f.write_str("x86-64"),
            Self::Other(name) => f.write_str(name),
            Self::Unknown => f.write_str("Unknown"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub address: u64,
    pub file_offset: usize,
    /// In-memory size; may exceed `file_size` for zero-filled storage.
    pub size: u64,
    pub file_size: usize,
    pub executable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub address: u64,
    pub file_offset: Option<usize>,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub offset: usize,
    pub address: u64,
    pub bytes: Vec<u8>,
    pub text: String,
    pub branch_target: Option<u64>,
    /// False means the row is an explicit undecodable data byte.
    pub valid: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntropyBin {
    pub offset: usize,
    pub size: usize,
    /// Shannon entropy in bits per byte, between zero and eight.
    pub entropy: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintableString {
    pub offset: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLocation {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub address: u64,
    pub file_offset: Option<usize>,
    /// Exclusive end of this line-table interval, never beyond its sequence.
    pub end_address: u64,
}

#[derive(Clone, Debug)]
struct Mapping {
    address: u64,
    offset: usize,
    size: usize,
}

#[derive(Clone, Debug)]
pub struct BinaryImage {
    bytes: Arc<[u8]>,
    pub format: BinaryFormat,
    pub architecture: Architecture,
    pub entry: Option<u64>,
    pub sections: Vec<Section>,
    pub symbols: Vec<Symbol>,
    pub source_locations: Vec<SourceLocation>,
    /// Explicit analysis limits and optional-debug-data errors.
    pub warnings: Vec<String>,
    mappings: Vec<Mapping>,
    source_end_prefix: Vec<u64>,
}

impl BinaryImage {
    pub fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err("Binary exceeds the 512 MiB inspection limit".into());
        }
        let format = detect_format(&bytes)?;
        if format == BinaryFormat::Raw {
            return Ok(Self {
                bytes: bytes.into(),
                format,
                architecture: Architecture::Unknown,
                entry: None,
                sections: Vec::new(),
                symbols: Vec::new(),
                source_locations: Vec::new(),
                warnings: vec!["Unrecognized format: bytes remain available; select an architecture before disassembly".into()],
                mappings: Vec::new(),
                source_end_prefix: Vec::new(),
            });
        }
        let object = object::File::parse(bytes.as_slice())
            .map_err(|error| format!("Cannot parse {format}: {error}"))?;
        if format == BinaryFormat::Pe
            && object
                .relative_address_base()
                .checked_add(u64::from(u32::MAX))
                .is_none()
        {
            return Err("PE image base cannot represent its relative virtual address space".into());
        }
        let architecture = match object.architecture() {
            object::Architecture::I386 => Architecture::X86,
            object::Architecture::X86_64 | object::Architecture::X86_64_X32 => Architecture::X86_64,
            object::Architecture::Unknown => Architecture::Unknown,
            other => Architecture::Other(format!("{other:?}")),
        };
        let mut sections = Vec::new();
        let mut mappings = Vec::new();
        let mut metadata_text_bytes = 0usize;
        for section in object.sections() {
            if sections.len() == MAX_SECTIONS {
                return Err("Binary exceeds the 16,384 section inspection limit".into());
            }
            let name = checked_name(section.name_bytes().map_err(|e| e.to_string())?)?;
            metadata_text_bytes += name.len();
            if metadata_text_bytes > MAX_METADATA_TEXT {
                return Err("Section names exceed the 16 MiB metadata text limit".into());
            }
            let (file_offset, file_size) = match section.file_range() {
                Some((offset, size)) => checked_file_range(offset, size, bytes.len())?,
                None => (0, 0),
            };
            let address = section.address();
            let size = section.size();
            address
                .checked_add(size)
                .ok_or_else(|| format!("Section {name} address range overflows"))?;
            let (mapped, executable) = section_properties(section.flags(), section.kind());
            let mapped_size = u64::try_from(file_size).unwrap_or(u64::MAX).min(size);
            if mapped && mapped_size != 0 {
                mappings.push(Mapping {
                    address,
                    offset: file_offset,
                    size: usize::try_from(mapped_size)
                        .map_err(|_| "Section mapping is too large".to_string())?,
                });
            }
            sections.push(Section {
                name,
                address,
                file_offset,
                size,
                file_size,
                executable,
            });
        }
        let mut warnings = Vec::new();
        if mappings.is_empty() {
            warnings.push(
                "No file-backed section address mappings; segment-only binaries are not yet mapped"
                    .into(),
            );
        }
        let mut symbols = Vec::new();
        for (count, symbol) in object.symbols().chain(object.dynamic_symbols()).enumerate() {
            if count == MAX_SYMBOLS {
                warnings.push("Symbol table truncated at 200,000 entries".into());
                break;
            }
            if symbol.is_undefined() {
                continue;
            }
            let Ok(name_bytes) = symbol.name_bytes() else {
                if !warnings
                    .iter()
                    .any(|warning| warning == "Malformed symbol names were omitted")
                {
                    warnings.push("Malformed symbol names were omitted".into());
                }
                continue;
            };
            if name_bytes.is_empty() {
                continue;
            }
            let name = checked_name(name_bytes)?;
            metadata_text_bytes += name.len();
            if metadata_text_bytes > MAX_METADATA_TEXT {
                warnings.push("Symbols truncated at the 16 MiB metadata text limit".into());
                break;
            }
            symbols.push(Symbol {
                name,
                address: symbol.address(),
                file_offset: address_to_offset(&mappings, symbol.address()),
                size: symbol.size(),
            });
        }
        symbols.sort_by(|a, b| (a.address, &a.name).cmp(&(b.address, &b.name)));
        symbols.dedup();
        let mut source_locations = if object.kind() == object::ObjectKind::Relocatable {
            warnings.push("Relocatable-object DWARF needs relocation processing; source mapping is unavailable".into());
            Vec::new()
        } else {
            dwarf::read_locations(&object, &mut warnings)
        };
        for location in &mut source_locations {
            location.file_offset = address_to_offset(&mappings, location.address);
        }
        source_locations.sort_by(|a, b| {
            (a.address, a.end_address, &a.path, a.line).cmp(&(
                b.address,
                b.end_address,
                &b.path,
                b.line,
            ))
        });
        source_locations.dedup();
        let source_end_prefix = source_end_prefix(&source_locations);
        let entry = (object.kind() != object::ObjectKind::Relocatable).then(|| object.entry());
        Ok(Self {
            bytes: bytes.into(),
            format,
            architecture,
            entry,
            sections,
            symbols,
            source_locations,
            warnings,
            mappings,
            source_end_prefix,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn offset_to_address(&self, offset: usize) -> Option<u64> {
        if offset >= self.bytes.len() {
            return None;
        }
        if self.format == BinaryFormat::Raw {
            return u64::try_from(offset).ok();
        }
        let address = offset_to_address(&self.mappings, offset)?;
        (address_to_offset(&self.mappings, address) == Some(offset)).then_some(address)
    }

    pub fn address_to_offset(&self, address: u64) -> Option<usize> {
        if self.format == BinaryFormat::Raw {
            let offset = usize::try_from(address).ok()?;
            return (offset < self.bytes.len()).then_some(offset);
        }
        address_to_offset(&self.mappings, address)
    }

    /// Lookup respects DWARF sequence ends and does not extend a source line
    /// through unreported code or data. Overlapping line tables are ambiguous.
    pub fn source_at_offset(&self, offset: usize) -> Option<&SourceLocation> {
        let address = self.offset_to_address(offset)?;
        let end = self
            .source_locations
            .partition_point(|row| row.address <= address);
        let mut matches = self.source_locations[..end]
            .iter()
            .enumerate()
            .rev()
            .take_while(|(index, _)| {
                self.source_end_prefix
                    .get(*index)
                    .is_some_and(|end| address < *end)
            })
            .map(|(_, row)| row)
            .filter(|row| address < row.end_address);
        let result = matches.next()?;
        if matches.any(|row| {
            row.path != result.path || row.line != result.line || row.column != result.column
        }) {
            return None;
        }
        Some(result)
    }

    pub fn disassemble(
        &self,
        offset: usize,
        max_bytes: usize,
        max_instructions: usize,
    ) -> Result<Vec<Instruction>, String> {
        self.disassemble_as(&self.architecture, offset, max_bytes, max_instructions)
    }

    /// Explicit architecture selection is useful for raw machine-code blobs.
    /// Recognized non-x86 images are never silently decoded as x86.
    pub fn disassemble_as(
        &self,
        architecture: &Architecture,
        offset: usize,
        max_bytes: usize,
        max_instructions: usize,
    ) -> Result<Vec<Instruction>, String> {
        let bitness = match architecture {
            Architecture::X86 => 32,
            Architecture::X86_64 => 64,
            other => {
                return Err(format!(
                    "Disassembly is unavailable for {other}; this decoder supports x86 and x86-64"
                ));
            }
        };
        if self.format != BinaryFormat::Raw && architecture != &self.architecture {
            return Err("Architecture override is only supported for raw bytes".into());
        }
        if offset > self.bytes.len() {
            return Err("Disassembly offset is outside the file".into());
        }
        if offset == self.bytes.len() || max_bytes == 0 || max_instructions == 0 {
            return Ok(Vec::new());
        }
        let address = self.offset_to_address(offset).ok_or_else(|| {
            "This offset has no unambiguous file-backed virtual address".to_string()
        })?;
        let mut count = (self.bytes.len() - offset)
            .min(max_bytes)
            .min(MAX_DECODE_BYTES);
        // Never let a single linear decode cross a mapping boundary or virtual
        // discontinuity, even if the file bytes are adjacent.
        if self.format != BinaryFormat::Raw {
            let mapping = self
                .mappings
                .iter()
                .find(|mapping| offset >= mapping.offset && offset - mapping.offset < mapping.size)
                .ok_or_else(|| "No mapped bytes at this offset".to_string())?;
            count = count.min(mapping.size - (offset - mapping.offset));
            // Overlapping mappings become ambiguous at the first overlap.
            for other in &self.mappings {
                if other.offset > offset && other.offset - offset < count {
                    count = other.offset - offset;
                }
                if other.address > address && other.address - address < count as u64 {
                    count = (other.address - address) as usize;
                }
            }
        }
        address
            .checked_add(count as u64)
            .ok_or_else(|| "Disassembly address range overflows".to_string())?;
        let data = &self.bytes[offset..offset + count];
        let mut decoder = Decoder::with_ip(bitness, data, address, DecoderOptions::NONE);
        let mut formatter = IntelFormatter::new();
        formatter.options_mut().set_hex_prefix("0x");
        formatter.options_mut().set_hex_suffix("");
        formatter.options_mut().set_uppercase_hex(false);
        let mut result = Vec::new();
        while decoder.can_decode() && result.len() < max_instructions.min(MAX_INSTRUCTIONS) {
            let start = decoder.position();
            let instruction = decoder.decode();
            let valid = !instruction.is_invalid();
            let mut text = String::new();
            let (length, branch_target) = if valid {
                formatter.format(&instruction, &mut text);
                let branch = (0..instruction.op_count())
                    .any(|i| {
                        matches!(
                            instruction.op_kind(i),
                            OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64
                        )
                    })
                    .then(|| instruction.near_branch_target());
                (instruction.len(), branch)
            } else {
                text = format!(
                    "db 0x{:02x} ; invalid or truncated instruction",
                    data[start]
                );
                decoder.set_position(start + 1).map_err(|e| e.to_string())?;
                decoder.set_ip(address + start as u64 + 1);
                (1, None)
            };
            result.push(Instruction {
                offset: offset + start,
                address: address + start as u64,
                bytes: data[start..start + length].to_vec(),
                text,
                branch_target,
                valid,
            });
        }
        Ok(result)
    }

    pub fn entropy(&self, range: Range<usize>, bin_size: usize) -> Result<Vec<EntropyBin>, String> {
        let data = self
            .bytes
            .get(range.clone())
            .ok_or("Entropy range is outside the file")?;
        if bin_size == 0 {
            return Err("Entropy bin size must be nonzero".into());
        }
        if data.len().div_ceil(bin_size) > MAX_ENTROPY_BINS {
            return Err("Entropy analysis is limited to 8,192 bins; increase the bin size".into());
        }
        Ok(data
            .chunks(bin_size)
            .enumerate()
            .map(|(index, chunk)| {
                let mut counts = [0usize; 256];
                for byte in chunk {
                    counts[*byte as usize] += 1;
                }
                let entropy = counts
                    .into_iter()
                    .filter(|count| *count > 0)
                    .map(|count| {
                        let probability = count as f64 / chunk.len() as f64;
                        -probability * probability.log2()
                    })
                    .sum();
                EntropyBin {
                    offset: range.start + index * bin_size,
                    size: chunk.len(),
                    entropy,
                }
            })
            .collect())
    }

    /// ASCII strings only; each result is bounded to 4,096 bytes. This does not
    /// claim to enumerate UTF-8/UTF-16 strings or infer program data types.
    pub fn strings(&self, min_len: usize, max_results: usize) -> Vec<PrintableString> {
        let mut result = Vec::new();
        let mut offset = 0;
        let limit = max_results.min(MAX_SYMBOLS);
        while offset < self.bytes.len() && result.len() < limit {
            if !(0x20..=0x7e).contains(&self.bytes[offset]) {
                offset += 1;
                continue;
            }
            let start = offset;
            while offset < self.bytes.len() && (0x20..=0x7e).contains(&self.bytes[offset]) {
                offset += 1;
            }
            if offset - start >= min_len.max(1) {
                let end = offset.min(start + MAX_NAME_BYTES);
                result.push(PrintableString {
                    offset: start,
                    text: String::from_utf8_lossy(&self.bytes[start..end]).into_owned(),
                });
            }
        }
        result
    }

    /// Fixed-length edits cannot move addresses. The new snapshot is parsed
    /// before return, so invalid structural edits leave the caller's image intact.
    pub fn patched(&self, offset: usize, replacement: &[u8]) -> Result<Self, String> {
        let end = offset
            .checked_add(replacement.len())
            .ok_or("Patch range overflows")?;
        if end > self.bytes.len() {
            return Err("Patch range is outside the file".into());
        }
        let mut bytes = self.bytes.to_vec();
        bytes[offset..end].copy_from_slice(replacement);
        Self::parse(bytes)
    }
}

fn detect_format(bytes: &[u8]) -> Result<BinaryFormat, String> {
    if bytes.starts_with(b"\x7fELF") {
        return Ok(BinaryFormat::Elf);
    }
    if bytes.starts_with(b"MZ") {
        return Ok(BinaryFormat::Pe);
    }
    match bytes.get(..4) {
        Some(
            [0xfe, 0xed, 0xfa, 0xce]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xcf, 0xfa, 0xed, 0xfe],
        ) => Ok(BinaryFormat::MachO),
        Some(
            [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
            | [0xca, 0xfe, 0xba, 0xbf]
            | [0xbf, 0xba, 0xfe, 0xca],
        ) => Err(
            "Universal/fat Mach-O needs an architecture slice; this format is not yet supported"
                .into(),
        ),
        _ => Ok(BinaryFormat::Raw),
    }
}

fn checked_name(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_NAME_BYTES {
        return Err("Binary metadata name exceeds 4,096 bytes".into());
    }
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn checked_file_range(offset: u64, size: u64, length: usize) -> Result<(usize, usize), String> {
    let offset = usize::try_from(offset).map_err(|_| "Section offset is too large")?;
    let size = usize::try_from(size).map_err(|_| "Section size is too large")?;
    if offset.checked_add(size).is_none_or(|end| end > length) {
        return Err("Section file range is outside the binary".into());
    }
    Ok((offset, size))
}

fn section_properties(flags: object::SectionFlags, kind: object::SectionKind) -> (bool, bool) {
    match flags {
        object::SectionFlags::Elf { sh_flags } => (
            sh_flags & u64::from(object::elf::SHF_ALLOC) != 0,
            sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0,
        ),
        object::SectionFlags::Coff { characteristics } => (
            characteristics
                & (object::pe::IMAGE_SCN_MEM_READ
                    | object::pe::IMAGE_SCN_MEM_WRITE
                    | object::pe::IMAGE_SCN_MEM_EXECUTE)
                != 0,
            characteristics & object::pe::IMAGE_SCN_MEM_EXECUTE != 0,
        ),
        object::SectionFlags::MachO { flags } => (
            flags & object::macho::S_ATTR_DEBUG == 0,
            flags
                & (object::macho::S_ATTR_PURE_INSTRUCTIONS
                    | object::macho::S_ATTR_SOME_INSTRUCTIONS)
                != 0,
        ),
        _ => (false, kind == object::SectionKind::Text),
    }
}

fn unique_mapping(mut mappings: impl Iterator<Item = u64>) -> Option<u64> {
    let result = mappings.next()?;
    // Even equal-address overlapping sections are reported as ambiguous.
    mappings.next().is_none().then_some(result)
}

fn address_to_offset(mappings: &[Mapping], address: u64) -> Option<usize> {
    let mut matches = mappings.iter().filter_map(|mapping| {
        let delta = usize::try_from(address.checked_sub(mapping.address)?).ok()?;
        (delta < mapping.size)
            .then(|| mapping.offset.checked_add(delta))
            .flatten()
    });
    let result = matches.next()?;
    (matches.next().is_none() && offset_to_address(mappings, result) == Some(address))
        .then_some(result)
}

fn offset_to_address(mappings: &[Mapping], offset: usize) -> Option<u64> {
    unique_mapping(mappings.iter().filter_map(|mapping| {
        let delta = offset.checked_sub(mapping.offset)?;
        (delta < mapping.size)
            .then(|| mapping.address.checked_add(delta as u64))
            .flatten()
    }))
}

fn source_end_prefix(locations: &[SourceLocation]) -> Vec<u64> {
    let mut largest_end = 0;
    locations
        .iter()
        .map(|location| {
            largest_end = largest_end.max(location.end_address);
            largest_end
        })
        .collect()
}

#[cfg(test)]
mod tests;
