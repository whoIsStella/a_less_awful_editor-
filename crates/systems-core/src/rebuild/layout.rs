//! Checked append-only file and virtual address assignments.
use super::{
    RebuildError, RebuildErrorCode, RebuildLimits,
    elf::ElfLayout,
    relocate::evaluate_pc32,
    unit::{RelocatableUnit, UnitSectionKind},
};
use std::{collections::BTreeMap, ops::Range};
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SectionPlacement {
    pub(super) index: u32,
    pub(super) kind: UnitSectionKind,
    pub(super) alignment: u64,
}
#[derive(Debug, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) struct AppendLayout {
    pub(super) placed_sections: Vec<SectionPlacement>,
    pub(super) section_addresses: BTreeMap<u32, u64>,
    pub(super) section_files: BTreeMap<u32, Range<usize>>,
    pub(super) body_address: u64,
    pub(super) phdr_count: usize,
    pub(super) phdr_file: Range<usize>,
    pub(super) phdr_address: u64,
    pub(super) read_file: Range<usize>,
    pub(super) read_address: u64,
    pub(super) text_file: Range<usize>,
    pub(super) note_file: Range<usize>,
    pub(super) strtab_file: Range<usize>,
    pub(super) symtab_file: Range<usize>,
    pub(super) shstrtab_file: Range<usize>,
    pub(super) shdr_file: Range<usize>,
    pub(super) output_len: usize,
    pub(super) detour: Vec<u8>,
    pub(super) changed_original_ranges: Vec<Range<usize>>,
}
fn invalid(detail: &str) -> RebuildError {
    RebuildError {
        code: RebuildErrorCode::InvalidLayout,
        detail: detail.into(),
    }
}
fn too_large() -> RebuildError {
    RebuildError {
        code: RebuildErrorCode::TooLarge,
        detail: "Rebuilt image exceeds profile".into(),
    }
}
fn align(value: usize, alignment: usize) -> Result<usize, RebuildError> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(invalid("Invalid layout alignment"));
    }
    value
        .checked_add(alignment - 1)
        .map(|v| v & !(alignment - 1))
        .ok_or_else(too_large)
}
fn range(start: usize, size: usize) -> Result<Range<usize>, RebuildError> {
    Ok(start..start.checked_add(size).ok_or_else(too_large)?)
}
#[allow(dead_code)]
pub(super) fn plan_layout(
    original: &ElfLayout,
    unit: &RelocatableUnit,
    limits: &RebuildLimits,
) -> Result<AppendLayout, RebuildError> {
    if original.program_headers.len() > limits.max_input_program_headers
        || original.file_len > limits.max_image_bytes
    {
        return Err(too_large());
    }
    let text = unit
        .sections
        .iter()
        .find(|s| s.kind == UnitSectionKind::Text)
        .ok_or_else(|| invalid("Missing replacement text"))?;
    let readonly: Vec<_> = unit
        .sections
        .iter()
        .filter(|s| s.kind == UnitSectionKind::ReadOnlyData)
        .collect();
    let section_count = original
        .section_count
        .checked_add(readonly.len())
        .and_then(|n| n.checked_add(2))
        .ok_or_else(too_large)?;
    if section_count > limits.max_section_headers || section_count >= 0xff00 {
        return Err(too_large());
    }
    let phdr_existing = original
        .program_headers
        .iter()
        .filter(|h| h.kind == 6)
        .count();
    if phdr_existing > 1 {
        return Err(invalid("Multiple program header segments"));
    }
    let phdr_count = original
        .program_headers
        .len()
        .checked_add(2 + usize::from(phdr_existing == 0))
        .ok_or_else(too_large)?;
    if phdr_count >= 0xffff {
        return Err(too_large());
    }
    let mut alignment = 4096usize;
    let mut max_load_end = 0u64;
    for h in &original.program_headers {
        if h.kind == 1 {
            alignment =
                alignment.max(usize::try_from(h.alignment.max(1)).map_err(|_| too_large())?);
            max_load_end =
                max_load_end.max(h.address.checked_add(h.memory_size).ok_or_else(too_large)?);
        }
    }
    for section in &unit.sections {
        alignment = alignment.max(usize::try_from(section.alignment).map_err(|_| too_large())?);
    }
    if alignment > limits.max_image_bytes || !alignment.is_power_of_two() {
        return Err(too_large());
    }
    let read_start = align(original.file_len, alignment)?;
    let read_address = align(
        usize::try_from(max_load_end).map_err(|_| too_large())?,
        alignment,
    )? as u64;
    let phdr_file = range(
        read_start,
        phdr_count.checked_mul(56).ok_or_else(too_large)?,
    )?;
    let mut cursor = phdr_file.end;
    let mut section_addresses = BTreeMap::new();
    let mut section_files = BTreeMap::new();
    for section in readonly {
        cursor = align(
            cursor,
            usize::try_from(section.alignment).map_err(|_| too_large())?,
        )?;
        let file = range(cursor, section.bytes.len())?;
        let address = read_address
            .checked_add((cursor - read_start) as u64)
            .ok_or_else(too_large)?;
        section_addresses.insert(section.index, address);
        section_files.insert(section.index, file.clone());
        cursor = file.end;
    }
    let read_file = read_start..cursor;
    let text_align = 4096usize.max(usize::try_from(text.alignment).map_err(|_| too_large())?);
    cursor = align(cursor, text_align)?;
    let text_file = range(cursor, text.bytes.len())?;
    let body_address = read_address
        .checked_add((cursor - read_start) as u64)
        .ok_or_else(too_large)?;
    section_addresses.insert(text.index, body_address);
    section_files.insert(text.index, text_file.clone());
    cursor = text_file.end;
    let note_file = range(align(cursor, 4)?, 52)?;
    cursor = note_file.end;
    let symbol_name_len = "__ale_body_"
        .len()
        .checked_add(original.site.name.len())
        .and_then(|n| n.checked_add(1))
        .ok_or_else(too_large)?;
    let strtab_file = range(
        cursor,
        original
            .strtab_size
            .checked_add(symbol_name_len)
            .ok_or_else(too_large)?,
    )?;
    cursor = strtab_file.end;
    let symtab_file = range(
        align(cursor, 8)?,
        original.symtab_size.checked_add(24).ok_or_else(too_large)?,
    )?;
    cursor = symtab_file.end;
    let mut names = original.shstrtab_size;
    names = names
        .checked_add(".ale.text.".len())
        .and_then(|n| n.checked_add(original.site.name.len() + 1))
        .and_then(|n| n.checked_add(".note.ale.rewrite".len() + 1))
        .ok_or_else(too_large)?;
    for section in unit
        .sections
        .iter()
        .filter(|s| s.kind == UnitSectionKind::ReadOnlyData)
    {
        names = names
            .checked_add(format!(".ale.rodata.{}", section.index).len() + 1)
            .ok_or_else(too_large)?;
    }
    let shstrtab_file = range(cursor, names)?;
    cursor = shstrtab_file.end;
    let shdr_file = range(
        align(cursor, 8)?,
        section_count.checked_mul(64).ok_or_else(too_large)?,
    )?;
    let output_len = shdr_file.end;
    if output_len > limits.max_image_bytes
        || read_start < original.file_len
        || read_address < max_load_end
    {
        return Err(too_large());
    }
    let branch_place = original
        .site
        .address
        .checked_add(1)
        .ok_or_else(|| invalid("Entry address overflow"))?;
    let branch = evaluate_pc32(body_address, -4, branch_place)?;
    if original.site.capacity < 5 {
        return Err(invalid("Entry span is too short"));
    }
    let mut detour = vec![0x90; original.site.capacity];
    detour[0] = 0xe9;
    detour[1..5].copy_from_slice(&branch);
    let entry_end = original
        .site
        .offset
        .checked_add(original.site.capacity)
        .ok_or_else(too_large)?;
    let mut placed_sections = vec![SectionPlacement {
        index: text.index,
        kind: text.kind,
        alignment: text.alignment,
    }];
    placed_sections.extend(
        unit.sections
            .iter()
            .filter(|s| s.kind == UnitSectionKind::ReadOnlyData)
            .map(|s| SectionPlacement {
                index: s.index,
                kind: s.kind,
                alignment: s.alignment,
            }),
    );
    Ok(AppendLayout {
        placed_sections,
        section_addresses,
        section_files,
        body_address,
        phdr_count,
        phdr_file,
        phdr_address: read_address,
        read_file,
        read_address,
        text_file,
        note_file,
        strtab_file,
        symtab_file,
        shstrtab_file,
        shdr_file,
        output_len,
        detour,
        changed_original_ranges: vec![
            0x20..0x28,
            0x28..0x30,
            0x38..0x3a,
            0x3c..0x3e,
            original.site.offset..entry_end,
        ],
    })
}
