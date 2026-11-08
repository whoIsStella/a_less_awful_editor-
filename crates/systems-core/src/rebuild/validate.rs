//! Read-back validation of a complete candidate against the immutable base.
use super::{
    RebuildError, RebuildErrorCode, elf::ElfLayout, layout::AppendLayout, metadata::decode_note,
    relocate::ResolvedUnit, unit::UnitSectionKind,
};
use crate::BinaryImage;
use object::{Object, ObjectSection, ObjectSymbol};
fn invalid(detail: &str) -> RebuildError {
    RebuildError {
        code: RebuildErrorCode::InvalidLayout,
        detail: detail.into(),
    }
}
fn read<const N: usize>(b: &[u8], o: usize) -> Result<[u8; N], RebuildError> {
    b.get(
        o..o.checked_add(N)
            .ok_or_else(|| invalid("Candidate offset overflow"))?,
    )
    .and_then(|s| s.try_into().ok())
    .ok_or_else(|| invalid("Truncated candidate"))
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
fn bytes(b: &[u8], r: std::ops::Range<usize>) -> Result<&[u8], RebuildError> {
    b.get(r)
        .ok_or_else(|| invalid("Candidate range outside file"))
}
#[cfg(test)]
pub(super) fn validate_candidate(
    base: &BinaryImage,
    original: &ElfLayout,
    plan: &AppendLayout,
    resolved: &ResolvedUnit,
    candidate: &[u8],
) -> Result<BinaryImage, RebuildError> {
    validate_candidate_with_cancel(
        base,
        original,
        plan,
        resolved,
        candidate,
        &std::sync::atomic::AtomicBool::new(false),
    )
}
pub(super) fn validate_candidate_with_cancel(
    base: &BinaryImage,
    original: &ElfLayout,
    plan: &AppendLayout,
    resolved: &ResolvedUnit,
    candidate: &[u8],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<BinaryImage, RebuildError> {
    check_cancel(cancel)?;
    if base.bytes().len() != original.file_len
        || candidate.len() != plan.output_len
        || candidate.len() < base.bytes().len()
    {
        return Err(invalid("Candidate length mismatch"));
    }
    let site_end = original
        .site
        .offset
        .checked_add(original.site.capacity)
        .ok_or_else(|| invalid("Entry range overflow"))?;
    let allowed = [
        0x20..0x28,
        0x28..0x30,
        0x38..0x3a,
        0x3c..0x3e,
        original.site.offset..site_end,
    ];
    for (i, (&old, &new)) in base.bytes().iter().zip(candidate).enumerate() {
        if i & 0xffff == 0 {
            check_cancel(cancel)?;
        }
        if old != new && !allowed.iter().any(|r| r.contains(&i)) {
            return Err(invalid("Unexpected modification in original image"));
        }
    }
    if bytes(candidate, original.site.offset..site_end)? != plan.detour
        || u64le(candidate, 32)? != plan.phdr_file.start as u64
        || u64le(candidate, 40)? != plan.shdr_file.start as u64
        || usize::from(u16le(candidate, 56)?) != plan.phdr_count
        || usize::from(u16le(candidate, 60)?)
            != original.section_count + plan.placed_sections.len() + 1
        || u16le(candidate, 62)? != u16le(base.bytes(), 62)?
    {
        return Err(invalid("Candidate header or entry differs from plan"));
    }
    if plan.phdr_file.len()
        != plan
            .phdr_count
            .checked_mul(56)
            .ok_or_else(|| invalid("Program header size overflow"))?
        || plan.phdr_file.start != plan.read_file.start
        || plan.phdr_address != plan.read_address
        || plan.phdr_file.end > plan.read_file.end
    {
        return Err(invalid(
            "Program header table is not mapped by the read-only segment",
        ));
    }
    let mut phdrs = Vec::with_capacity(plan.phdr_count);
    for i in 0..plan.phdr_count {
        let offset = plan
            .phdr_file
            .start
            .checked_add(i * 56)
            .ok_or_else(|| invalid("Program header offset overflow"))?;
        phdrs.push(read::<56>(candidate, offset)?);
    }
    let word =
        |h: &[u8; 56], o: usize| u32::from_le_bytes(h[o..o + 4].try_into().expect("fixed header"));
    let quad =
        |h: &[u8; 56], o: usize| u64::from_le_bytes(h[o..o + 8].try_into().expect("fixed header"));
    let first = phdrs
        .first()
        .ok_or_else(|| invalid("Missing program header segment"))?;
    if word(first, 0) != 6
        || word(first, 4) != 4
        || quad(first, 8) != plan.phdr_file.start as u64
        || quad(first, 16) != plan.phdr_address
        || quad(first, 32) != plan.phdr_file.len() as u64
        || quad(first, 40) != plan.phdr_file.len() as u64
        || quad(first, 24) != plan.phdr_address
        || quad(first, 48) != 8
    {
        return Err(invalid("Program header segment mismatch"));
    }
    let old_offset = usize::try_from(u64le(base.bytes(), 32)?)
        .map_err(|_| invalid("Old program offset overflow"))?;
    let mut old_headers = Vec::new();
    for i in 0..original.program_headers.len() {
        let h = read::<56>(base.bytes(), old_offset + i * 56)?;
        if word(&h, 0) != 6 {
            old_headers.push(h);
        }
    }
    if phdrs.len() != old_headers.len() + 3 || phdrs[1..1 + old_headers.len()] != old_headers {
        return Err(invalid("Original program headers were altered"));
    }
    let new_read = &phdrs[phdrs.len() - 2];
    let new_text = &phdrs[phdrs.len() - 1];
    for (h, flags, file, address) in [
        (new_read, 4, &plan.read_file, plan.read_address),
        (new_text, 5, &plan.text_file, plan.body_address),
    ] {
        if word(h, 0) != 1
            || word(h, 4) != flags
            || quad(h, 8) != file.start as u64
            || quad(h, 16) != address
            || quad(h, 24) != address
            || quad(h, 32) != file.len() as u64
            || quad(h, 40) != file.len() as u64
            || quad(h, 48) != 4096
            || address % 4096 != file.start as u64 % 4096
        {
            return Err(invalid("New load segment does not match plan"));
        }
    }
    let mut last_end = 0u64;
    let mut load_seen = false;
    let mut interp_seen = false;
    for h in &phdrs {
        let kind = word(h, 0);
        let flags = word(h, 4);
        let offset = quad(h, 8);
        let address = quad(h, 16);
        let filesz = quad(h, 32);
        let memsz = quad(h, 40);
        let align = quad(h, 48);
        if kind == 3 {
            if load_seen || interp_seen {
                return Err(invalid("Invalid interpreter segment ordering"));
            }
            interp_seen = true;
        }
        if kind == 1 {
            let end = address
                .checked_add(memsz)
                .ok_or_else(|| invalid("Load segment range overflow"))?;
            if flags & 3 == 3
                || filesz > memsz
                || address < last_end
                || offset
                    .checked_add(filesz)
                    .is_none_or(|e| e > candidate.len() as u64)
                || (align > 1 && (!align.is_power_of_two() || offset % align != address % align))
            {
                return Err(invalid("Invalid candidate load segments"));
            }
            last_end = end;
            load_seen = true;
        }
    }
    validate_metadata(base.bytes(), original, plan, candidate, cancel)?;
    let file = object::File::parse(candidate).map_err(|e| RebuildError {
        code: RebuildErrorCode::InvalidLayout,
        detail: e.to_string(),
    })?;
    for section in &plan.placed_sections {
        let name = if section.kind == UnitSectionKind::Text {
            format!(".ale.text.{}", original.site.name)
        } else {
            format!(".ale.rodata.{}", section.index)
        };
        let section_file = plan
            .section_files
            .get(&section.index)
            .ok_or_else(|| invalid("Missing planned section"))?;
        let address = *plan
            .section_addresses
            .get(&section.index)
            .ok_or_else(|| invalid("Missing planned section address"))?;
        let parsed = file
            .section_by_name(&name)
            .ok_or_else(|| invalid("Missing candidate section"))?;
        let data = resolved
            .sections
            .get(&section.index)
            .ok_or_else(|| invalid("Missing relocated section"))?;
        if parsed.address() != address
            || parsed.file_range() != Some((section_file.start as u64, section_file.len() as u64))
            || bytes(candidate, section_file.clone())? != data
            || parsed.size() != section_file.len() as u64
            || parsed.align() != section.alignment
            || !matches!(parsed.flags(),object::SectionFlags::Elf {sh_flags} if sh_flags==if section.kind==UnitSectionKind::Text {6} else {2})
        {
            return Err(invalid("Candidate section differs from planned bytes"));
        }
        let load = if section.kind == UnitSectionKind::Text {
            new_text
        } else {
            new_read
        };
        let delta = address
            .checked_sub(quad(load, 16))
            .ok_or_else(|| invalid("Section below mapped segment"))?;
        if quad(load, 8).checked_add(delta) != Some(section_file.start as u64)
            || delta
                .checked_add(section_file.len() as u64)
                .is_none_or(|v| v > quad(load, 32))
        {
            return Err(invalid("Section and load mappings disagree"));
        }
    }
    let note = file
        .section_by_name(".note.ale.rewrite")
        .ok_or_else(|| invalid("Missing rewrite note"))?;
    if note.kind() != object::SectionKind::Note
        || note.align() != 4
        || !matches!(note.flags(), object::SectionFlags::Elf { sh_flags: 0 })
        || note.file_range() != Some((plan.note_file.start as u64, plan.note_file.len() as u64))
    {
        return Err(invalid("Rewrite note range mismatch"));
    }
    let location = decode_note(bytes(candidate, plan.note_file.clone())?)?;
    if location.entry_address != original.site.address
        || location.original_capacity != original.site.capacity as u64
        || location.body_address != plan.body_address
        || location.body_size != plan.text_file.len() as u64
    {
        return Err(invalid("Rewrite note contents mismatch"));
    }
    let body_name = format!("__ale_body_{}", original.site.name);
    if !file.symbols().any(|s| {
        s.name().ok() == Some(body_name.as_str())
            && s.address() == plan.body_address
            && s.size() == plan.text_file.len() as u64
    }) {
        return Err(invalid("Missing body symbol"));
    }
    let parsed = BinaryImage::parse(candidate.to_vec()).map_err(|detail| RebuildError {
        code: RebuildErrorCode::InvalidLayout,
        detail,
    })?;
    for section in &plan.placed_sections {
        let address = plan.section_addresses[&section.index];
        let file = &plan.section_files[&section.index];
        if !file.is_empty()
            && (parsed.address_to_offset(address) != Some(file.start)
                || parsed.address_to_offset(address + file.len() as u64 - 1) != Some(file.end - 1))
        {
            return Err(invalid("Reparsed section mapping differs"));
        }
    }
    Ok(parsed)
}

fn validate_metadata(
    base: &[u8],
    original: &ElfLayout,
    plan: &AppendLayout,
    candidate: &[u8],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), RebuildError> {
    check_cancel(cancel)?;
    let old_table =
        usize::try_from(u64le(base, 40)?).map_err(|_| invalid("Old section table overflow"))?;
    let count = original.section_count;
    let shstr = usize::from(u16le(base, 62)?);
    let mut sym = None;
    for i in 0..count {
        if u32le(base, old_table + i * 64 + 4)? == 2 && sym.replace(i).is_some() {
            return Err(invalid("Multiple symbol tables"));
        }
    }
    let sym = sym.ok_or_else(|| invalid("Missing symbol table"))?;
    let sym_header = old_table + sym * 64;
    let str_index = usize::try_from(u32le(base, sym_header + 40)?)
        .map_err(|_| invalid("String index overflow"))?;
    if str_index >= count || shstr >= count || str_index == shstr {
        return Err(invalid("Invalid string table index"));
    }
    let str_header = old_table + str_index * 64;
    let name_header = old_table + shstr * 64;
    let old_slice = |header: usize| -> Result<&[u8], RebuildError> {
        let start = usize::try_from(u64le(base, header + 24)?)
            .map_err(|_| invalid("Metadata offset overflow"))?;
        let len = usize::try_from(u64le(base, header + 32)?)
            .map_err(|_| invalid("Metadata length overflow"))?;
        bytes(
            base,
            start
                ..start
                    .checked_add(len)
                    .ok_or_else(|| invalid("Metadata range overflow"))?,
        )
    };
    let old_strings = old_slice(str_header)?;
    let old_symbols = old_slice(sym_header)?;
    let old_names = old_slice(name_header)?;
    let local = usize::try_from(u32le(base, sym_header + 44)?)
        .map_err(|_| invalid("Local count overflow"))?;
    let split = local
        .checked_mul(24)
        .ok_or_else(|| invalid("Local count overflow"))?;
    if split > old_symbols.len() {
        return Err(invalid("Invalid original symbol boundary"));
    }
    let mut expected_strings = old_strings.to_vec();
    let body_name = format!("__ale_body_{}", original.site.name);
    expected_strings.extend_from_slice(body_name.as_bytes());
    expected_strings.push(0);
    if bytes(candidate, plan.strtab_file.clone())? != expected_strings {
        return Err(invalid("Symbol string table changed unexpectedly"));
    }
    let mut body = [0u8; 24];
    body[..4].copy_from_slice(
        &u32::try_from(old_strings.len())
            .map_err(|_| invalid("Symbol name overflow"))?
            .to_le_bytes(),
    );
    body[4] = 2;
    body[6..8].copy_from_slice(
        &u16::try_from(count)
            .map_err(|_| invalid("Section index overflow"))?
            .to_le_bytes(),
    );
    body[8..16].copy_from_slice(&plan.body_address.to_le_bytes());
    body[16..24].copy_from_slice(&(plan.text_file.len() as u64).to_le_bytes());
    let mut expected_symbols = Vec::with_capacity(old_symbols.len() + 24);
    expected_symbols.extend_from_slice(&old_symbols[..split]);
    expected_symbols.extend_from_slice(&body);
    expected_symbols.extend_from_slice(&old_symbols[split..]);
    if bytes(candidate, plan.symtab_file.clone())? != expected_symbols {
        return Err(invalid("Static symbols changed unexpectedly"));
    }
    let mut expected_names = old_names.to_vec();
    for section in &plan.placed_sections {
        let name = if section.kind == UnitSectionKind::Text {
            format!(".ale.text.{}", original.site.name)
        } else {
            format!(".ale.rodata.{}", section.index)
        };
        expected_names.extend_from_slice(name.as_bytes());
        expected_names.push(0);
    }
    expected_names.extend_from_slice(b".note.ale.rewrite\0");
    if bytes(candidate, plan.shstrtab_file.clone())? != expected_names {
        return Err(invalid("Section names changed unexpectedly"));
    }
    let mut name_offset = old_names.len();
    for (index, section) in plan.placed_sections.iter().enumerate() {
        let name = if section.kind == UnitSectionKind::Text {
            format!(".ale.text.{}", original.site.name)
        } else {
            format!(".ale.rodata.{}", section.index)
        };
        let file = plan
            .section_files
            .get(&section.index)
            .ok_or_else(|| invalid("Missing planned section range"))?;
        let address = *plan
            .section_addresses
            .get(&section.index)
            .ok_or_else(|| invalid("Missing planned section address"))?;
        let mut expected = [0u8; 64];
        expected[..4].copy_from_slice(
            &u32::try_from(name_offset)
                .map_err(|_| invalid("Section name overflow"))?
                .to_le_bytes(),
        );
        expected[4..8].copy_from_slice(&1u32.to_le_bytes());
        let flags = if section.kind == UnitSectionKind::Text {
            6u64
        } else {
            2
        };
        expected[8..16].copy_from_slice(&flags.to_le_bytes());
        expected[16..24].copy_from_slice(&address.to_le_bytes());
        expected[24..32].copy_from_slice(&(file.start as u64).to_le_bytes());
        expected[32..40].copy_from_slice(&(file.len() as u64).to_le_bytes());
        expected[48..56].copy_from_slice(&section.alignment.to_le_bytes());
        if read::<64>(candidate, plan.shdr_file.start + (count + index) * 64)? != expected {
            return Err(invalid("Injected section header differs from plan"));
        }
        name_offset = name_offset
            .checked_add(name.len() + 1)
            .ok_or_else(|| invalid("Section name overflow"))?;
    }
    let mut note_header = [0u8; 64];
    note_header[..4].copy_from_slice(
        &u32::try_from(name_offset)
            .map_err(|_| invalid("Section name overflow"))?
            .to_le_bytes(),
    );
    note_header[4..8].copy_from_slice(&7u32.to_le_bytes());
    note_header[24..32].copy_from_slice(&(plan.note_file.start as u64).to_le_bytes());
    note_header[32..40].copy_from_slice(&(plan.note_file.len() as u64).to_le_bytes());
    note_header[48..56].copy_from_slice(&4u64.to_le_bytes());
    if read::<64>(
        candidate,
        plan.shdr_file.start + (count + plan.placed_sections.len()) * 64,
    )? != note_header
    {
        return Err(invalid("Rewrite note section header differs from plan"));
    }
    for i in 0..count {
        let mut expected = read::<64>(base, old_table + i * 64)?;
        if i == sym {
            expected[24..32].copy_from_slice(&(plan.symtab_file.start as u64).to_le_bytes());
            expected[32..40].copy_from_slice(&(plan.symtab_file.len() as u64).to_le_bytes());
            expected[44..48].copy_from_slice(
                &u32::try_from(local + 1)
                    .map_err(|_| invalid("Local count overflow"))?
                    .to_le_bytes(),
            );
        } else if i == str_index {
            expected[24..32].copy_from_slice(&(plan.strtab_file.start as u64).to_le_bytes());
            expected[32..40].copy_from_slice(&(plan.strtab_file.len() as u64).to_le_bytes());
        } else if i == shstr {
            expected[24..32].copy_from_slice(&(plan.shstrtab_file.start as u64).to_le_bytes());
            expected[32..40].copy_from_slice(&(plan.shstrtab_file.len() as u64).to_le_bytes());
        }
        if read::<64>(candidate, plan.shdr_file.start + i * 64)? != expected {
            return Err(invalid("Original section header changed unexpectedly"));
        }
    }
    let mut occupied = vec![
        plan.phdr_file.clone(),
        plan.note_file.clone(),
        plan.strtab_file.clone(),
        plan.symtab_file.clone(),
        plan.shstrtab_file.clone(),
        plan.shdr_file.clone(),
    ];
    occupied.extend(plan.section_files.values().cloned());
    occupied.sort_by_key(|r| r.start);
    let mut cursor = base.len();
    for r in occupied {
        if r.start < cursor || r.end < r.start || r.end > candidate.len() {
            return Err(invalid("Appended ranges overlap or exceed output"));
        }
        for chunk in bytes(candidate, cursor..r.start)?.chunks(65536) {
            check_cancel(cancel)?;
            if chunk.iter().any(|&x| x != 0) {
                return Err(invalid("Unexpected bytes in appended padding"));
            }
        }
        cursor = r.end;
    }
    if cursor != candidate.len() {
        return Err(invalid("Unaccounted appended bytes"));
    }
    Ok(())
}

fn check_cancel(cancel: &std::sync::atomic::AtomicBool) -> Result<(), RebuildError> {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        Err(RebuildError {
            code: RebuildErrorCode::Cancelled,
            detail: "Candidate validation cancelled".into(),
        })
    } else {
        Ok(())
    }
}
