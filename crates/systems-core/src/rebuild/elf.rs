//! Admission of images into the conservative relocated-rebuild profile.
use super::{RebuildError, RebuildErrorCode, RebuildLimits};
use crate::{BinaryImage, recompile::ReplacementSite};
use object::{Object, ObjectSection, ObjectSymbol};

#[derive(Debug)]
#[allow(dead_code)]
pub(super) struct ElfLayout {
    pub(super) program_headers: Vec<ProgramHeader>,
    pub(super) dynamic_relocations: Vec<std::ops::Range<u64>>,
    pub(super) definitions: Vec<super::relocate::Definition>,
    pub(super) file_len: usize,
    pub(super) section_count: usize,
    pub(super) shstrtab_size: usize,
    pub(super) symtab_size: usize,
    pub(super) strtab_size: usize,
    pub(super) site: ReplacementSite,
}
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(super) struct ProgramHeader {
    pub(super) kind: u32,
    pub(super) flags: u32,
    pub(super) offset: u64,
    pub(super) address: u64,
    pub(super) file_size: u64,
    pub(super) memory_size: u64,
    pub(super) alignment: u64,
}
fn error(code: RebuildErrorCode, detail: &str) -> RebuildError {
    RebuildError {
        code,
        detail: detail.into(),
    }
}
fn read<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], RebuildError> {
    bytes
        .get(
            offset
                ..offset
                    .checked_add(N)
                    .ok_or_else(|| error(RebuildErrorCode::InvalidLayout, "ELF offset overflow"))?,
        )
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| error(RebuildErrorCode::InvalidLayout, "Truncated ELF header"))
}
fn u16le(b: &[u8], o: usize) -> Result<u16, RebuildError> {
    Ok(u16::from_le_bytes(read(b, o)?))
}
fn u32le(b: &[u8], o: usize) -> Result<u32, RebuildError> {
    Ok(u32::from_le_bytes(read(b, o)?))
}
fn u64le(b: &[u8], o: usize) -> Result<u64, RebuildError> {
    Ok(u64::from_le_bytes(read(b, o)?))
}
fn table(bytes: &[u8], offset: u64, count: usize, size: usize) -> Result<usize, RebuildError> {
    let start = usize::try_from(offset)
        .map_err(|_| error(RebuildErrorCode::InvalidLayout, "ELF table offset overflow"))?;
    let end = start
        .checked_add(
            count
                .checked_mul(size)
                .ok_or_else(|| error(RebuildErrorCode::InvalidLayout, "ELF table size overflow"))?,
        )
        .ok_or_else(|| error(RebuildErrorCode::InvalidLayout, "ELF table end overflow"))?;
    if end > bytes.len() {
        return Err(error(
            RebuildErrorCode::InvalidLayout,
            "ELF table outside image",
        ));
    }
    Ok(start)
}
#[allow(dead_code)]
pub(super) fn inspect_image(
    image: &BinaryImage,
    site: &ReplacementSite,
    limits: &RebuildLimits,
) -> Result<ElfLayout, RebuildError> {
    let b = image.bytes();
    if b.len() > limits.max_image_bytes {
        return Err(error(
            RebuildErrorCode::TooLarge,
            "Image exceeds relocated profile limit",
        ));
    }
    if b.get(..7) != Some(b"\x7fELF\x02\x01\x01")
        || !matches!(b.get(7), Some(0 | 3))
        || !matches!(u16le(b, 16)?, 2 | 3)
        || u16le(b, 18)? != 62
        || u32le(b, 20)? != 1
    {
        return Err(error(
            RebuildErrorCode::UnsupportedImage,
            "Expected a Linux x86-64 ELF executable",
        ));
    }
    if u16le(b, 52)? != 64 || u16le(b, 54)? != 56 || u16le(b, 58)? != 64 {
        return Err(error(
            RebuildErrorCode::InvalidLayout,
            "Unsupported ELF header entry sizes",
        ));
    }
    let phnum = usize::from(u16le(b, 56)?);
    let shnum = usize::from(u16le(b, 60)?);
    let shstr = usize::from(u16le(b, 62)?);
    if phnum == 0xffff || shnum == 0 || shstr == 0xffff || shstr >= shnum {
        return Err(error(
            RebuildErrorCode::UnsupportedImage,
            "Extended or invalid ELF numbering",
        ));
    }
    if phnum > limits.max_input_program_headers || shnum > limits.max_section_headers {
        return Err(error(
            RebuildErrorCode::TooLarge,
            "ELF table count exceeds profile",
        ));
    }
    let phoff = table(b, u64le(b, 32)?, phnum, 56)?;
    let shoff = table(b, u64le(b, 40)?, shnum, 64)?;
    let mut static_symtabs = Vec::new();
    let mut dynamic_symtabs = 0usize;
    for i in 0..shnum {
        let o = shoff + i * 64;
        let kind = u32le(b, o + 4)?;
        if kind == 18 {
            return Err(error(
                RebuildErrorCode::UnsupportedMetadata,
                "Extended symbol indices are unsupported",
            ));
        }
        if kind == 2 || kind == 11 {
            let size = u64le(b, o + 32)?;
            let entsize = u64le(b, o + 56)?;
            if entsize != 24 || size % entsize != 0 {
                return Err(error(
                    RebuildErrorCode::InvalidLayout,
                    "Invalid symbol table entry size",
                ));
            }
            if size / entsize > limits.max_symbols as u64 {
                return Err(error(
                    RebuildErrorCode::TooLarge,
                    "Symbol table exceeds profile",
                ));
            }
            table(
                b,
                u64le(b, o + 24)?,
                usize::try_from(size / entsize)
                    .map_err(|_| error(RebuildErrorCode::TooLarge, "Symbol count overflows"))?,
                24,
            )?;
            if kind == 2 {
                static_symtabs.push(i);
                if static_symtabs.len() > 1 {
                    return Err(error(
                        RebuildErrorCode::UnsupportedMetadata,
                        "Multiple static symbol tables are unsupported",
                    ));
                }
            } else {
                dynamic_symtabs += 1;
                if dynamic_symtabs > 1 {
                    return Err(error(
                        RebuildErrorCode::UnsupportedMetadata,
                        "Multiple dynamic symbol tables are unsupported",
                    ));
                }
            }
        }
    }
    for i in 0..shnum {
        let o = shoff + i * 64;
        let link = usize::try_from(u32le(b, o + 40)?)
            .map_err(|_| error(RebuildErrorCode::InvalidLayout, "Section link overflow"))?;
        if link != 0 && static_symtabs.contains(&link) {
            return Err(error(
                RebuildErrorCode::UnsupportedMetadata,
                "Static symbol-index consumers cannot yet be preserved",
            ));
        }
    }
    let sym_index = *static_symtabs.first().ok_or_else(|| {
        error(
            RebuildErrorCode::UnsupportedMetadata,
            "A static symbol table is required",
        )
    })?;
    let sym_offset = shoff + sym_index * 64;
    let str_index = usize::try_from(u32le(b, sym_offset + 40)?).map_err(|_| {
        error(
            RebuildErrorCode::InvalidLayout,
            "String table index overflow",
        )
    })?;
    if str_index >= shnum
        || str_index == shstr
        || u32le(b, shoff + str_index * 64 + 4)? != 3
        || u32le(b, shoff + shstr * 64 + 4)? != 3
        || u64le(b, sym_offset + 8)? & 2 != 0
        || u64le(b, shoff + str_index * 64 + 8)? & 2 != 0
        || u64le(b, shoff + shstr * 64 + 8)? & 2 != 0
    {
        return Err(error(
            RebuildErrorCode::UnsupportedMetadata,
            "Invalid symbol or section string table",
        ));
    }
    let size_for = |index: usize| -> Result<usize, RebuildError> {
        let o = shoff + index * 64;
        let size = usize::try_from(u64le(b, o + 32)?)
            .map_err(|_| error(RebuildErrorCode::TooLarge, "Metadata size overflows"))?;
        table(b, u64le(b, o + 24)?, size, 1)?;
        Ok(size)
    };
    let symtab_size = size_for(sym_index)?;
    let strtab_size = size_for(str_index)?;
    let shstrtab_size = size_for(shstr)?;
    let actual = ReplacementSite::discover(image, site.address).map_err(|d| RebuildError {
        code: RebuildErrorCode::InvalidLayout,
        detail: d,
    })?;
    if actual.name != site.name
        || actual.offset != site.offset
        || actual.capacity != site.capacity
        || actual.address != site.address
        || site.capacity < 5
    {
        return Err(error(
            RebuildErrorCode::InvalidLayout,
            "Invalid or too short selected function",
        ));
    }
    let end = site
        .address
        .checked_add(site.capacity as u64)
        .ok_or_else(|| error(RebuildErrorCode::InvalidLayout, "Function address overflow"))?;
    let mut headers = Vec::with_capacity(phnum);
    let mut last_end = None;
    let mut file_ranges = Vec::<std::ops::Range<u64>>::new();
    let mut selected = false;
    for i in 0..phnum {
        let o = phoff + i * 56;
        let h = ProgramHeader {
            kind: u32le(b, o)?,
            flags: u32le(b, o + 4)?,
            offset: u64le(b, o + 8)?,
            address: u64le(b, o + 16)?,
            file_size: u64le(b, o + 32)?,
            memory_size: u64le(b, o + 40)?,
            alignment: u64le(b, o + 48)?,
        };
        if h.kind == 4 || h.kind == 0x6474e553 {
            let off = usize::try_from(h.offset)
                .map_err(|_| error(RebuildErrorCode::InvalidLayout, "Note offset overflows"))?;
            let len = usize::try_from(h.file_size)
                .map_err(|_| error(RebuildErrorCode::InvalidLayout, "Note length overflows"))?;
            let end = off
                .checked_add(len)
                .ok_or_else(|| error(RebuildErrorCode::InvalidLayout, "Note range overflows"))?;
            check_notes(b.get(off..end).ok_or_else(|| {
                error(RebuildErrorCode::InvalidLayout, "Note range outside image")
            })?)?;
        }
        if h.kind == 1 {
            let file_end = h.offset.checked_add(h.file_size).ok_or_else(|| {
                error(
                    RebuildErrorCode::InvalidLayout,
                    "Segment file range overflow",
                )
            })?;
            let mem_end = h.address.checked_add(h.memory_size).ok_or_else(|| {
                error(RebuildErrorCode::InvalidLayout, "Segment address overflow")
            })?;
            if h.file_size > h.memory_size
                || file_end > b.len() as u64
                || h.flags & 3 == 3
                || (h.alignment > 1
                    && (!h.alignment.is_power_of_two()
                        || h.offset % h.alignment != h.address % h.alignment))
                || last_end.is_some_and(|v| h.address < v)
                || (h.file_size != 0
                    && file_ranges
                        .iter()
                        .any(|r| h.offset < r.end && file_end > r.start))
            {
                return Err(error(
                    RebuildErrorCode::InvalidLayout,
                    "Invalid, overlapping or writable-executable load segment",
                ));
            }
            last_end = Some(mem_end);
            if h.file_size != 0 {
                file_ranges.push(h.offset..file_end);
            }
            if h.flags & 1 != 0
                && site.address >= h.address
                && end <= h.address.checked_add(h.file_size).unwrap_or(0)
                && h.offset.checked_add(site.address - h.address) == Some(site.offset as u64)
            {
                selected = true;
            }
        }
        headers.push(h);
    }
    if u16le(b, 16)? == 3 {
        validate_pie_identity(b, &headers)?;
    }
    if !selected {
        return Err(error(
            RebuildErrorCode::InvalidLayout,
            "Function is not in a matching executable load segment",
        ));
    }
    let dynamic_relocations =
        super::dynamic::inspect_dynamic(b, &headers, shoff, shnum, limits.max_symbols)?;
    if dynamic_relocations
        .iter()
        .any(|range| range.start < end && range.end > site.address)
    {
        return Err(error(
            RebuildErrorCode::UnsupportedMetadata,
            "Dynamic relocation overlaps selected function",
        ));
    }
    let file = object::File::parse(b).map_err(|e| RebuildError {
        code: RebuildErrorCode::InvalidLayout,
        detail: e.to_string(),
    })?;
    let selected_end = site.offset.checked_add(site.capacity).ok_or_else(|| {
        error(
            RebuildErrorCode::InvalidLayout,
            "Selected file range overflow",
        )
    })?;
    let selected_range = site.offset..selected_end;
    let overlaps =
        |a: &std::ops::Range<usize>, b: &std::ops::Range<usize>| a.start < b.end && b.start < a.end;
    let phdr_end = phoff
        .checked_add(phnum.checked_mul(56).ok_or_else(|| {
            error(
                RebuildErrorCode::InvalidLayout,
                "Program header size overflow",
            )
        })?)
        .ok_or_else(|| {
            error(
                RebuildErrorCode::InvalidLayout,
                "Program header end overflow",
            )
        })?;
    let shdr_end = shoff
        .checked_add(shnum.checked_mul(64).ok_or_else(|| {
            error(
                RebuildErrorCode::InvalidLayout,
                "Section header size overflow",
            )
        })?)
        .ok_or_else(|| {
            error(
                RebuildErrorCode::InvalidLayout,
                "Section header end overflow",
            )
        })?;
    if overlaps(&selected_range, &(0..64))
        || overlaps(&selected_range, &(phoff..phdr_end))
        || overlaps(&selected_range, &(shoff..shdr_end))
    {
        return Err(error(
            RebuildErrorCode::Overlap,
            "Selected function overlaps ELF header tables",
        ));
    }
    let owner = file
        .symbols()
        .find(|s| {
            s.kind() == object::SymbolKind::Text
                && s.address() == site.address
                && s.name().ok() == Some(site.name.as_str())
        })
        .and_then(|s| s.section_index())
        .ok_or_else(|| {
            error(
                RebuildErrorCode::InvalidLayout,
                "Selected symbol has no section",
            )
        })?;
    for section in file.sections() {
        if section.index() == owner {
            continue;
        }
        if let Some((offset, size)) = section.file_range() {
            let start = usize::try_from(offset)
                .map_err(|_| error(RebuildErrorCode::InvalidLayout, "Section offset overflow"))?;
            let end = start
                .checked_add(
                    usize::try_from(size).map_err(|_| {
                        error(RebuildErrorCode::InvalidLayout, "Section size overflow")
                    })?,
                )
                .ok_or_else(|| error(RebuildErrorCode::InvalidLayout, "Section range overflow"))?;
            if size > 0 && overlaps(&selected_range, &(start..end)) {
                return Err(error(
                    RebuildErrorCode::Overlap,
                    "Selected function overlaps another section",
                ));
            }
        }
    }
    for symbol in file.symbols().chain(file.dynamic_symbols()) {
        if !symbol.is_undefined()
            && symbol.kind() == object::SymbolKind::Text
            && symbol.address() == site.address
            && symbol.size() > 0
            && symbol.size() != site.capacity as u64
        {
            return Err(error(
                RebuildErrorCode::InvalidLayout,
                "Function aliases disagree on extent",
            ));
        }
        if !symbol.is_undefined() && symbol.address() > site.address && symbol.address() < end {
            return Err(error(
                RebuildErrorCode::InvalidLayout,
                "An existing symbol points inside the selected span",
            ));
        }
    }
    let mut eh_frame_seen = false;
    let mut inspected_bytes = 0usize;
    for section in file.sections() {
        if matches!(
            section.kind(),
            object::SectionKind::Text | object::SectionKind::Note
        ) || section.name().ok() == Some(".eh_frame")
        {
            let size = usize::try_from(section.size())
                .map_err(|_| error(RebuildErrorCode::TooLarge, "Section size exceeds profile"))?;
            inspected_bytes = inspected_bytes
                .checked_add(size)
                .ok_or_else(|| error(RebuildErrorCode::TooLarge, "Inspected sections overflow"))?;
            if inspected_bytes > b.len() {
                return Err(error(
                    RebuildErrorCode::TooLarge,
                    "Inspected sections exceed the bounded image size",
                ));
            }
        }
        if section.kind() == object::SectionKind::Text {
            let data = section.data().map_err(|e| RebuildError {
                code: RebuildErrorCode::InvalidLayout,
                detail: e.to_string(),
            })?;
            let mut decoder = iced_x86::Decoder::with_ip(
                64,
                data,
                section.address(),
                iced_x86::DecoderOptions::NONE,
            );
            while decoder.can_decode() {
                let ins = decoder.decode();
                if ins.ip() >= site.address && ins.ip() < end {
                    continue;
                }
                if (0..ins.op_count()).any(|i| {
                    matches!(
                        ins.op_kind(i),
                        iced_x86::OpKind::NearBranch16
                            | iced_x86::OpKind::NearBranch32
                            | iced_x86::OpKind::NearBranch64
                    )
                }) {
                    let target = ins.near_branch_target();
                    if target > site.address && target < end {
                        return Err(error(
                            RebuildErrorCode::InvalidLayout,
                            "A known direct branch targets the selected span interior",
                        ));
                    }
                }
            }
        }
        if section.kind() == object::SectionKind::Note {
            let data = section.data().map_err(|e| RebuildError {
                code: RebuildErrorCode::UnsupportedMetadata,
                detail: e.to_string(),
            })?;
            check_notes(data)?;
        }
        if section.name().ok() == Some(".eh_frame") {
            if eh_frame_seen {
                return Err(error(
                    RebuildErrorCode::UnsupportedMetadata,
                    "Multiple unwind sections are unsupported",
                ));
            }
            eh_frame_seen = true;
            let data = section.uncompressed_data().map_err(|e| RebuildError {
                code: RebuildErrorCode::UnsupportedMetadata,
                detail: e.to_string(),
            })?;
            check_unwind(&data, section.address(), site.address..end)?;
        }
        if section
            .name()
            .ok()
            .is_some_and(|name| name.starts_with(".note.ale.rewrite"))
        {
            return Err(error(
                RebuildErrorCode::UnsupportedMetadata,
                "A previous relocated rewrite is present",
            ));
        }
    }
    let mut definitions = Vec::new();
    for symbol in file.symbols().chain(file.dynamic_symbols()) {
        if symbol.is_undefined() {
            continue;
        }
        let object::SymbolFlags::Elf { st_info, st_other } = symbol.flags() else {
            return Err(error(
                RebuildErrorCode::UnsupportedMetadata,
                "Non-ELF symbol flags",
            ));
        };
        let kind = match st_info & 15 {
            1 => super::relocate::DefinitionKind::Data,
            2 => super::relocate::DefinitionKind::Function,
            _ => continue,
        };
        let Some(index) = symbol.section_index() else {
            continue;
        };
        let section = file.section_by_index(index).map_err(|e| RebuildError {
            code: RebuildErrorCode::UnsupportedMetadata,
            detail: e.to_string(),
        })?;
        let object::SectionFlags::Elf { sh_flags } = section.flags() else {
            return Err(error(
                RebuildErrorCode::UnsupportedMetadata,
                "Non-ELF section flags",
            ));
        };
        let start = symbol.address();
        let span = if kind == super::relocate::DefinitionKind::Function {
            symbol.size().max(1)
        } else {
            symbol.size()
        };
        let end = start.checked_add(span);
        let relative = start.checked_sub(section.address());
        let file_offset = section.file_range().and_then(|(off, size)| {
            relative.and_then(|r| {
                r.checked_add(span)
                    .filter(|&end| end <= size)
                    .and_then(|_| off.checked_add(r))
            })
        });
        let mapping = end.and_then(|end| {
            headers.iter().find(|h| {
                h.kind == 1
                    && start >= h.address
                    && h.address.checked_add(h.file_size).is_some_and(|v| end <= v)
                    && h.offset.checked_add(start - h.address) == file_offset
            })
        });
        let pos = dynamic_relocations.partition_point(|r| r.end <= start);
        let dynamic_relocated =
            end.is_none_or(|end| dynamic_relocations.get(pos).is_some_and(|r| r.start < end));
        definitions.push(super::relocate::Definition {
            name: symbol
                .name()
                .map_err(|e| RebuildError {
                    code: RebuildErrorCode::UnsupportedMetadata,
                    detail: e.to_string(),
                })?
                .to_owned(),
            address: start,
            size: symbol.size(),
            kind,
            binding: st_info >> 4,
            visibility: st_other & 3,
            file_backed: span > 0
                && file_offset.is_some()
                && mapping.is_some()
                && sh_flags & 2 != 0,
            readable: mapping.is_some_and(|h| h.flags & 4 != 0),
            executable: sh_flags & 4 != 0 && mapping.is_some_and(|h| h.flags & 1 != 0),
            writable: sh_flags & 1 != 0 && mapping.is_some_and(|h| h.flags & 2 != 0),
            dynamic_relocated,
        });
    }
    if headers.iter().any(|h| h.kind == 0x6474e550) && !eh_frame_seen {
        return Err(error(
            RebuildErrorCode::UnsupportedMetadata,
            "Unwind index exists without a readable unwind section",
        ));
    }
    Ok(ElfLayout {
        program_headers: headers,
        dynamic_relocations,
        definitions,
        file_len: b.len(),
        section_count: shnum,
        shstrtab_size,
        symtab_size,
        strtab_size,
        site: site.clone(),
    })
}

pub(super) fn check_unwind(
    data: &[u8],
    address: u64,
    selected: std::ops::Range<u64>,
) -> Result<(), RebuildError> {
    use gimli::{BaseAddresses, CieOrFde, EhFrame, LittleEndian, UnwindSection};
    let frame = EhFrame::new(data, LittleEndian);
    let bases = BaseAddresses::default().set_eh_frame(address);
    let mut entries = frame.entries(&bases);
    let malformed = |e: gimli::Error| RebuildError {
        code: RebuildErrorCode::UnsupportedMetadata,
        detail: format!("Cannot establish unwind coverage: {e}"),
    };
    while let Some(entry) = entries.next().map_err(malformed)? {
        match entry {
            CieOrFde::Cie(cie) => {
                let mut instructions = cie.instructions(&frame, &bases);
                while instructions.next().map_err(malformed)?.is_some() {}
            }
            CieOrFde::Fde(partial) => {
                let fde = partial.parse(EhFrame::cie_from_offset).map_err(malformed)?;
                let start = fde.initial_address();
                let end = start.checked_add(fde.len()).ok_or_else(|| {
                    error(
                        RebuildErrorCode::UnsupportedMetadata,
                        "Unwind range overflows",
                    )
                })?;
                if start < selected.end && end > selected.start {
                    return Err(error(
                        RebuildErrorCode::UnsupportedMetadata,
                        "Selected function overlaps existing unwind metadata",
                    ));
                }
                let mut instructions = fde.instructions(&frame, &bases);
                while instructions.next().map_err(malformed)?.is_some() {}
            }
        }
    }
    Ok(())
}

pub(super) fn check_notes(bytes: &[u8]) -> Result<(), RebuildError> {
    let invalid = || {
        error(
            RebuildErrorCode::UnsupportedMetadata,
            "Malformed ELF note or GNU property",
        )
    };
    let align = |n: usize, a: usize| {
        n.checked_add(a - 1)
            .map(|x| x & !(a - 1))
            .ok_or_else(invalid)
    };
    let word = |data: &[u8], pos: usize| -> Result<u32, RebuildError> {
        let slice = data
            .get(pos..pos.checked_add(4).ok_or_else(invalid)?)
            .ok_or_else(invalid)?;
        Ok(u32::from_le_bytes(slice.try_into().map_err(|_| invalid())?))
    };
    let mut at = 0usize;
    while at < bytes.len() {
        let name_len = word(bytes, at)? as usize;
        let desc_len = word(bytes, at.checked_add(4).ok_or_else(invalid)?)? as usize;
        let kind = word(bytes, at.checked_add(8).ok_or_else(invalid)?)?;
        let name_start = at.checked_add(12).ok_or_else(invalid)?;
        let name_end = name_start.checked_add(name_len).ok_or_else(invalid)?;
        let desc_start = align(name_end, 4)?;
        let desc_end = desc_start.checked_add(desc_len).ok_or_else(invalid)?;
        let next = align(desc_end, 4)?;
        let name = bytes.get(name_start..name_end).ok_or_else(invalid)?;
        let desc = bytes.get(desc_start..desc_end).ok_or_else(invalid)?;
        if next > bytes.len() {
            return Err(invalid());
        }
        if name == b"GNU\0" && kind == 5 {
            let mut p = 0;
            while p < desc.len() {
                let typ = word(desc, p)?;
                let len = word(desc, p.checked_add(4).ok_or_else(invalid)?)? as usize;
                let begin = p.checked_add(8).ok_or_else(invalid)?;
                let end = begin.checked_add(len).ok_or_else(invalid)?;
                let data = desc.get(begin..end).ok_or_else(invalid)?;
                if typ == 0xc0000002 {
                    if data.len() != 4 {
                        return Err(invalid());
                    }
                    if word(data, 0)? & 3 != 0 {
                        return Err(error(
                            RebuildErrorCode::UnsupportedMetadata,
                            "CET-protected images cannot be relocated",
                        ));
                    }
                }
                p = align(end, 8)?;
                if p > desc.len() {
                    return Err(invalid());
                }
            }
        }
        at = next;
    }
    Ok(())
}

#[cfg(test)]
pub(super) use super::writer::write_candidate;

fn validate_pie_identity(bytes: &[u8], headers: &[ProgramHeader]) -> Result<(), RebuildError> {
    let bad = || {
        error(
            RebuildErrorCode::UnsupportedImage,
            "PIE rebuilding requires a main executable with a valid interpreter and PIE dynamic flag",
        )
    };
    let mut interp = None;
    let mut load_seen = false;
    for h in headers {
        if h.kind == 1 {
            load_seen = true;
        }
        if h.kind == 3 && (load_seen || interp.replace(h).is_some()) {
            return Err(bad());
        }
    }
    let interp = interp.ok_or_else(bad)?;
    let start = usize::try_from(interp.offset).map_err(|_| bad())?;
    let len = usize::try_from(interp.file_size).map_err(|_| bad())?;
    let path = bytes
        .get(start..start.checked_add(len).ok_or_else(bad)?)
        .ok_or_else(bad)?;
    if path.len() < 2 || path.last() != Some(&0) || path[..path.len() - 1].contains(&0) {
        return Err(bad());
    }
    let mut dynamic = headers.iter().filter(|h| h.kind == 2);
    let segment = dynamic.next().ok_or_else(bad)?;
    if dynamic.next().is_some() || segment.file_size % 16 != 0 {
        return Err(bad());
    }
    let start = usize::try_from(segment.offset).map_err(|_| bad())?;
    let len = usize::try_from(segment.file_size).map_err(|_| bad())?;
    let data = bytes
        .get(start..start.checked_add(len).ok_or_else(bad)?)
        .ok_or_else(bad)?;
    let mut flags = None;
    let mut terminated = false;
    for record in data.as_chunks::<16>().0.iter() {
        let tag = u64::from_le_bytes(record[..8].try_into().map_err(|_| bad())?);
        let value = u64::from_le_bytes(record[8..].try_into().map_err(|_| bad())?);
        if tag == 0 {
            terminated = true;
            break;
        }
        if tag == 0x6ffffffb && flags.replace(value).is_some() {
            return Err(bad());
        }
    }
    if !terminated || flags.is_none_or(|v| v & 0x08000000 == 0) {
        return Err(bad());
    }
    Ok(())
}
