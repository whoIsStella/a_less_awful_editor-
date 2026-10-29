//! Pure placement checks for the first native C function replacement path.
//! Preserves addresses; does not prove the recovered ABI or program semantics.
use crate::BinaryImage;
use iced_x86::{Decoder, DecoderOptions, FlowControl};
use object::{Object, ObjectSection, ObjectSymbol};

#[derive(Clone, Debug)]
pub struct ReplacementSite {
    pub name: String,
    pub address: u64,
    pub offset: usize,
    pub capacity: usize,
}
impl ReplacementSite {
    pub fn discover(image: &BinaryImage, address: u64) -> Result<Self, String> {
        let file = object::File::parse(image.bytes()).map_err(|e| e.to_string())?;
        if file.format() != object::BinaryFormat::Elf
            || !file.is_64()
            || !file.is_little_endian()
            || file.architecture() != object::Architecture::X86_64
            || !matches!(
                file.kind(),
                object::ObjectKind::Executable | object::ObjectKind::Dynamic
            )
            || !matches!(image.bytes().get(7), Some(0 | 3))
        {
            return Err("Function replacement currently requires a Linux/System V x86-64 ELF executable or shared object.".into());
        }
        let mut candidates = file.symbols().filter(|s| {
            s.kind() == object::SymbolKind::Text && s.address() == address && s.size() > 0
        });
        let symbol=candidates.next().ok_or("This function needs an exact ELF text symbol with a nonzero size; inferred boundaries are not sufficient for replacement.")?;
        if candidates.any(|s| s.size() != symbol.size()) {
            return Err("Function aliases disagree on size.".into());
        }
        let name = symbol.name().map_err(|e| e.to_string())?.to_owned();
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .enumerate()
                .all(|(i, c)| c == b'_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit())
        {
            return Err("The function symbol is not a supported C identifier.".into());
        }
        let capacity = usize::try_from(symbol.size()).map_err(|_| "Function size overflows")?;
        if capacity == 0 || capacity > 65536 {
            return Err("Replacement capacity must be between 1 and 65536 bytes.".into());
        }
        let end = address
            .checked_add(symbol.size())
            .ok_or("Function address overflows")?;
        let section = file
            .section_by_index(symbol.section_index().ok_or("Function has no section")?)
            .map_err(|e| e.to_string())?;
        if section.kind() != object::SectionKind::Text {
            return Err("Function is not in a text section.".into());
        }
        if section.relocations().next().is_some()
            || file
                .dynamic_relocations()
                .is_some_and(|mut relocations| relocations.any(|(at, _)| at >= address && at < end))
        {
            return Err("Original text relocations require a relocation-aware writer; this function cannot use fixed-span replacement.".into());
        }
        if address < section.address()
            || end
                > section
                    .address()
                    .checked_add(section.size())
                    .ok_or("Section range overflows")?
            || image.mappings.len() > 256
        {
            return Err(
                "Function exceeds its text section or the 256-mapping replacement profile.".into(),
            );
        }
        for other in file
            .symbols()
            .filter(|s| s.kind() == object::SymbolKind::Text)
        {
            if other.address() != address
                && other.address() < end
                && other
                    .address()
                    .checked_add(other.size().max(1))
                    .is_some_and(|e| e > address)
            {
                return Err("Overlapping function symbols prevent replacement.".into());
            }
        }
        let offset = image
            .address_to_offset(address)
            .ok_or("Function entry is not unambiguously file backed")?;
        for i in 0..capacity {
            if image.address_to_offset(address + i as u64) != offset.checked_add(i) {
                return Err("Function mapping is not contiguous and unambiguous.".into());
            }
        }
        Ok(Self {
            name,
            address,
            offset,
            capacity,
        })
    }
    /// Extract only a standalone function: no relocations, external calls, data
    /// sections or out-of-range branches. Shorter code is padded with x86 NOPs.
    pub fn replacement(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        if bytes.len() > 2 * 1024 * 1024 {
            return Err("Compiler object exceeds 2 MiB.".into());
        }
        let file = object::File::parse(bytes).map_err(|e| e.to_string())?;
        if file.format() != object::BinaryFormat::Elf
            || file.kind() != object::ObjectKind::Relocatable
            || !file.is_64()
            || !file.is_little_endian()
            || file.architecture() != object::Architecture::X86_64
        {
            return Err("Compiler did not produce an x86-64 ELF relocatable object.".into());
        }
        let mut matches = file.symbols().filter(|s| {
            s.name().ok() == Some(self.name.as_str())
                && s.kind() == object::SymbolKind::Text
                && !s.is_undefined()
        });
        let symbol = matches
            .next()
            .ok_or("Edited C must define the selected function with its original name.")?;
        if matches.next().is_some() {
            return Err("Duplicate replacement function symbol.".into());
        }
        let index = symbol.section_index().ok_or("Replacement has no section")?;
        let section = file.section_by_index(index).map_err(|e| e.to_string())?;
        let code = section.data().map_err(|e| e.to_string())?;
        if section.kind() != object::SectionKind::Text
            || symbol.address() != 0
            || symbol.size() != code.len() as u64
            || code.is_empty()
        {
            return Err("Replacement must occupy one complete standalone text section.".into());
        }
        if code.len() > self.capacity {
            return Err(format!(
                "Compiled function needs {} bytes; this symbol has {}. Function growth and relocation are not implemented yet.",
                code.len(),
                self.capacity
            ));
        }
        for s in file.sections() {
            if s.relocations().next().is_some() {
                return Err("Replacement contains relocations. Calls/globals/literals needing relocation are not supported yet.".into());
            }
            if s.index() != index
                && s.size() > 0
                && matches!(s.flags(),object::SectionFlags::Elf{sh_flags} if sh_flags & 2 != 0)
            {
                return Err("Replacement adds another allocated section. Extra functions, globals and constant pools are not supported yet.".into());
            }
        }
        let mut decoder = Decoder::with_ip(64, code, self.address, DecoderOptions::NONE);
        let mut instructions = Vec::new();
        while decoder.can_decode() {
            let instruction = decoder.decode();
            if instruction.is_invalid() {
                return Err("Compiled bytes contain an invalid instruction.".into());
            }
            instructions.push(instruction);
        }
        let boundaries: std::collections::BTreeSet<_> =
            instructions.iter().map(|i| i.ip()).collect();
        for instruction in &instructions {
            match instruction.flow_control() {
                FlowControl::Call|FlowControl::IndirectCall|FlowControl::IndirectBranch => return Err("Only standalone leaf-function replacements are supported in this first path.".into()),
                FlowControl::ConditionalBranch|FlowControl::UnconditionalBranch => {
                    if !boundaries.contains(&instruction.near_branch_target()) {return Err("Replacement branch leaves its instruction boundaries.".into());}
                },
                _=>(),
            }
            if instruction.is_ip_rel_memory_operand() {
                return Err("RIP-relative data references need relocation support.".into());
            }
        }
        if instructions.last().is_some_and(|i| {
            !matches!(
                i.flow_control(),
                FlowControl::Return | FlowControl::UnconditionalBranch | FlowControl::Exception
            )
        }) {
            return Err("Replacement falls through beyond its code.".into());
        }
        let mut result = code.to_vec();
        result.resize(self.capacity, 0x90);
        Ok(result)
    }
}
