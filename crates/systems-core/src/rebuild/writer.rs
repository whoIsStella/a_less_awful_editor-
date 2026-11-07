//! Minimal append-only writer for the admitted ELF64 profile.
use super::{
    RebuildError, RebuildErrorCode,
    elf::ElfLayout,
    layout::AppendLayout,
    metadata::{RewriteLocation, encode_note},
    relocate::ResolvedUnit,
    unit::UnitSectionKind,
};
use crate::BinaryImage;
use std::ops::Range;
fn invalid(detail: &str) -> RebuildError {
    RebuildError {
        code: RebuildErrorCode::InvalidLayout,
        detail: detail.into(),
    }
}
fn read<const N: usize>(b: &[u8], o: usize) -> Result<[u8; N], RebuildError> {
    b.get(o..o.checked_add(N).ok_or_else(|| invalid("Offset overflow"))?)
        .and_then(|x| x.try_into().ok())
        .ok_or_else(|| invalid("Truncated input"))
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
fn put(b: &mut [u8], o: usize, data: &[u8]) -> Result<(), RebuildError> {
    b.get_mut(
        o..o.checked_add(data.len())
            .ok_or_else(|| invalid("Output offset overflow"))?,
    )
    .ok_or_else(|| invalid("Output range outside candidate"))?
    .copy_from_slice(data);
    Ok(())
}
fn put16(b: &mut [u8], o: usize, v: u16) -> Result<(), RebuildError> {
    put(b, o, &v.to_le_bytes())
}
fn put32(b: &mut [u8], o: usize, v: u32) -> Result<(), RebuildError> {
    put(b, o, &v.to_le_bytes())
}
fn put64(b: &mut [u8], o: usize, v: u64) -> Result<(), RebuildError> {
    put(b, o, &v.to_le_bytes())
}
fn get(b: &[u8], offset: u64, size: u64) -> Result<&[u8], RebuildError> {
    let start = usize::try_from(offset).map_err(|_| invalid("Input offset overflow"))?;
    let size = usize::try_from(size).map_err(|_| invalid("Input size overflow"))?;
    b.get(
        start
            ..start
                .checked_add(size)
                .ok_or_else(|| invalid("Input range overflow"))?,
    )
    .ok_or_else(|| invalid("Input range outside image"))
}
fn at(range: &Range<usize>, offset: usize) -> Result<usize, RebuildError> {
    let x = range
        .start
        .checked_add(offset)
        .ok_or_else(|| invalid("Output index overflow"))?;
    if x > range.end {
        Err(invalid("Output index outside range"))
    } else {
        Ok(x)
    }
}
fn phdr(
    kind: u32,
    flags: u32,
    offset: usize,
    address: u64,
    size: usize,
    alignment: u64,
) -> Result<[u8; 56], RebuildError> {
    let mut h = [0u8; 56];
    put32(&mut h, 0, kind)?;
    put32(&mut h, 4, flags)?;
    put64(&mut h, 8, offset as u64)?;
    put64(&mut h, 16, address)?;
    put64(&mut h, 24, address)?;
    put64(&mut h, 32, size as u64)?;
    put64(&mut h, 40, size as u64)?;
    put64(&mut h, 48, alignment)?;
    Ok(h)
}
fn section_header(
    name: u32,
    kind: u32,
    flags: u64,
    address: u64,
    file: &Range<usize>,
    alignment: u64,
) -> Result<[u8; 64], RebuildError> {
    let mut h = [0u8; 64];
    put32(&mut h, 0, name)?;
    put32(&mut h, 4, kind)?;
    put64(&mut h, 8, flags)?;
    put64(&mut h, 16, address)?;
    put64(&mut h, 24, file.start as u64)?;
    put64(&mut h, 32, file.len() as u64)?;
    put64(&mut h, 48, alignment)?;
    Ok(h)
}
fn append_name(names: &mut Vec<u8>, name: &str) -> Result<u32, RebuildError> {
    let pos = u32::try_from(names.len()).map_err(|_| invalid("Section name table overflow"))?;
    names.extend_from_slice(name.as_bytes());
    names.push(0);
    Ok(pos)
}
#[cfg(test)]
pub(super) fn write_candidate(
    image: &BinaryImage,
    original: &ElfLayout,
    placement: &AppendLayout,
    resolved: &ResolvedUnit,
) -> Result<Vec<u8>, RebuildError> {
    write_candidate_with_cancel(
        image,
        original,
        placement,
        resolved,
        &std::sync::atomic::AtomicBool::new(false),
    )
}
pub(super) fn write_candidate_with_cancel(
    image: &BinaryImage,
    original: &ElfLayout,
    placement: &AppendLayout,
    resolved: &ResolvedUnit,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Vec<u8>, RebuildError> {
    check_cancel(cancel)?;
    let base = image.bytes();
    if base.len() != original.file_len || placement.output_len < base.len() {
        return Err(invalid("Input does not match layout"));
    }
    let old_phoff =
        usize::try_from(u64le(base, 32)?).map_err(|_| invalid("Program table offset overflow"))?;
    let old_shoff =
        usize::try_from(u64le(base, 40)?).map_err(|_| invalid("Section table offset overflow"))?;
    let old_phnum = usize::from(u16le(base, 56)?);
    let old_shnum = usize::from(u16le(base, 60)?);
    let shstr = usize::from(u16le(base, 62)?);
    if old_phnum != original.program_headers.len()
        || old_shnum != original.section_count
        || shstr >= old_shnum
    {
        return Err(invalid("Header counts do not match layout"));
    }
    let mut sym_index = None;
    for i in 0..old_shnum {
        if u32le(base, old_shoff + i * 64 + 4)? == 2 && sym_index.replace(i).is_some() {
            return Err(invalid("Multiple static symbol tables"));
        }
    }
    let sym_index = sym_index.ok_or_else(|| invalid("Missing static symbol table"))?;
    let sym_header = old_shoff + sym_index * 64;
    let str_index = usize::try_from(u32le(base, sym_header + 40)?)
        .map_err(|_| invalid("String index overflow"))?;
    if str_index >= old_shnum
        || str_index == shstr
        || u32le(base, old_shoff + str_index * 64 + 4)? != 3
    {
        return Err(invalid("Invalid symbol string table"));
    }
    let shstr_header = old_shoff + shstr * 64;
    let str_header = old_shoff + str_index * 64;
    let old_names = get(
        base,
        u64le(base, shstr_header + 24)?,
        u64le(base, shstr_header + 32)?,
    )?;
    let old_strings = get(
        base,
        u64le(base, str_header + 24)?,
        u64le(base, str_header + 32)?,
    )?;
    let old_symbols = get(
        base,
        u64le(base, sym_header + 24)?,
        u64le(base, sym_header + 32)?,
    )?;
    let local_count = usize::try_from(u32le(base, sym_header + 44)?)
        .map_err(|_| invalid("Symbol count overflow"))?;
    if old_symbols.len() % 24 != 0 || local_count == 0 || local_count > old_symbols.len() / 24 {
        return Err(invalid("Invalid local symbol boundary"));
    }
    for (i, symbol) in old_symbols.chunks_exact(24).enumerate() {
        let local = symbol[4] >> 4 == 0;
        if local != (i < local_count) {
            return Err(invalid("Invalid static symbol ordering"));
        }
    }
    let mut candidate = vec![0u8; placement.output_len];
    put_chunks(&mut candidate, 0, base, cancel)?;
    put64(&mut candidate, 32, placement.phdr_file.start as u64)?;
    put64(&mut candidate, 40, placement.shdr_file.start as u64)?;
    put16(
        &mut candidate,
        56,
        u16::try_from(placement.phdr_count).map_err(|_| invalid("Program count overflow"))?,
    )?;
    let new_shnum = old_shnum
        .checked_add(placement.placed_sections.len() + 1)
        .ok_or_else(|| invalid("Section count overflow"))?;
    put16(
        &mut candidate,
        60,
        u16::try_from(new_shnum).map_err(|_| invalid("Section count overflow"))?,
    )?;
    put(&mut candidate, original.site.offset, &placement.detour)?;
    let phdr_len = placement.phdr_file.len();
    let mut phdrs = Vec::with_capacity(placement.phdr_count);
    phdrs.push(phdr(
        6,
        4,
        placement.phdr_file.start,
        placement.phdr_address,
        phdr_len,
        8,
    )?);
    for i in 0..old_phnum {
        let offset = old_phoff
            .checked_add(i * 56)
            .ok_or_else(|| invalid("Program offset overflow"))?;
        let raw = read::<56>(base, offset)?;
        if u32::from_le_bytes(raw[..4].try_into().map_err(|_| invalid("Program header"))?) != 6 {
            phdrs.push(raw);
        }
    }
    phdrs.push(phdr(
        1,
        4,
        placement.read_file.start,
        placement.read_address,
        placement.read_file.len(),
        4096,
    )?);
    phdrs.push(phdr(
        1,
        5,
        placement.text_file.start,
        placement.body_address,
        placement.text_file.len(),
        4096,
    )?);
    if phdrs.len() != placement.phdr_count {
        return Err(invalid("Program header count mismatch"));
    }
    for (i, h) in phdrs.iter().enumerate() {
        put(&mut candidate, at(&placement.phdr_file, i * 56)?, h)?;
    }
    for section in &placement.placed_sections {
        let file = placement
            .section_files
            .get(&section.index)
            .ok_or_else(|| invalid("Missing section file range"))?;
        let data = resolved
            .sections
            .get(&section.index)
            .ok_or_else(|| invalid("Missing relocated section"))?;
        if data.len() != file.len() {
            return Err(invalid("Relocated section length mismatch"));
        }
        put_chunks(&mut candidate, file.start, data, cancel)?;
    }
    let note = encode_note(&RewriteLocation {
        entry_address: original.site.address,
        original_capacity: original.site.capacity as u64,
        body_address: placement.body_address,
        body_size: placement.text_file.len() as u64,
    });
    if note.len() != placement.note_file.len() {
        return Err(invalid("Note size mismatch"));
    }
    put(&mut candidate, placement.note_file.start, &note)?;
    let mut strings = old_strings.to_vec();
    let body_name = u32::try_from(strings.len()).map_err(|_| invalid("Symbol strings overflow"))?;
    strings.extend_from_slice(format!("__ale_body_{}", original.site.name).as_bytes());
    strings.push(0);
    if strings.len() != placement.strtab_file.len() {
        return Err(invalid("Symbol strings length mismatch"));
    }
    put(&mut candidate, placement.strtab_file.start, &strings)?;
    let split = local_count * 24;
    let mut symbols = Vec::with_capacity(old_symbols.len() + 24);
    symbols.extend_from_slice(&old_symbols[..split]);
    let mut body = [0u8; 24];
    put32(&mut body, 0, body_name)?;
    body[4] = 2;
    put16(
        &mut body,
        6,
        u16::try_from(old_shnum).map_err(|_| invalid("Body section index overflow"))?,
    )?;
    put64(&mut body, 8, placement.body_address)?;
    put64(&mut body, 16, placement.text_file.len() as u64)?;
    symbols.extend_from_slice(&body);
    symbols.extend_from_slice(&old_symbols[split..]);
    if symbols.len() != placement.symtab_file.len() {
        return Err(invalid("Symbol table length mismatch"));
    }
    put(&mut candidate, placement.symtab_file.start, &symbols)?;
    let mut names = old_names.to_vec();
    let mut new_headers = Vec::with_capacity(placement.placed_sections.len() + 1);
    for section in &placement.placed_sections {
        let name = if section.kind == UnitSectionKind::Text {
            format!(".ale.text.{}", original.site.name)
        } else {
            format!(".ale.rodata.{}", section.index)
        };
        let name = append_name(&mut names, &name)?;
        let file = placement
            .section_files
            .get(&section.index)
            .ok_or_else(|| invalid("Missing section range"))?;
        let address = *placement
            .section_addresses
            .get(&section.index)
            .ok_or_else(|| invalid("Missing section address"))?;
        let flags = if section.kind == UnitSectionKind::Text {
            6
        } else {
            2
        };
        new_headers.push(section_header(
            name,
            1,
            flags,
            address,
            file,
            section.alignment,
        )?);
    }
    let note_name = append_name(&mut names, ".note.ale.rewrite")?;
    new_headers.push(section_header(note_name, 7, 0, 0, &placement.note_file, 4)?);
    if names.len() != placement.shstrtab_file.len() {
        return Err(invalid("Section names length mismatch"));
    }
    put(&mut candidate, placement.shstrtab_file.start, &names)?;
    let old_headers = get(base, old_shoff as u64, (old_shnum * 64) as u64)?;
    put(&mut candidate, placement.shdr_file.start, old_headers)?;
    let sym_out = at(&placement.shdr_file, sym_index * 64)?;
    put64(
        &mut candidate,
        sym_out + 24,
        placement.symtab_file.start as u64,
    )?;
    put64(
        &mut candidate,
        sym_out + 32,
        placement.symtab_file.len() as u64,
    )?;
    put32(
        &mut candidate,
        sym_out + 44,
        u32::try_from(local_count + 1).map_err(|_| invalid("Local symbol count overflow"))?,
    )?;
    let str_out = at(&placement.shdr_file, str_index * 64)?;
    put64(
        &mut candidate,
        str_out + 24,
        placement.strtab_file.start as u64,
    )?;
    put64(
        &mut candidate,
        str_out + 32,
        placement.strtab_file.len() as u64,
    )?;
    let names_out = at(&placement.shdr_file, shstr * 64)?;
    put64(
        &mut candidate,
        names_out + 24,
        placement.shstrtab_file.start as u64,
    )?;
    put64(
        &mut candidate,
        names_out + 32,
        placement.shstrtab_file.len() as u64,
    )?;
    for (i, h) in new_headers.iter().enumerate() {
        put(
            &mut candidate,
            at(&placement.shdr_file, (old_shnum + i) * 64)?,
            h,
        )?;
    }
    Ok(candidate)
}

fn check_cancel(cancel: &std::sync::atomic::AtomicBool) -> Result<(), RebuildError> {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        Err(RebuildError {
            code: RebuildErrorCode::Cancelled,
            detail: "Candidate writing cancelled".into(),
        })
    } else {
        Ok(())
    }
}
fn put_chunks(
    output: &mut [u8],
    offset: usize,
    data: &[u8],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), RebuildError> {
    for (i, chunk) in data.chunks(65536).enumerate() {
        check_cancel(cancel)?;
        let at = offset
            .checked_add(
                i.checked_mul(65536)
                    .ok_or_else(|| invalid("Output offset overflow"))?,
            )
            .ok_or_else(|| invalid("Output offset overflow"))?;
        put(output, at, chunk)?;
    }
    Ok(())
}
