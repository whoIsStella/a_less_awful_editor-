//! Independent validation of original ELF dynamic-relocation metadata.
use super::{RebuildError, RebuildErrorCode};
use std::ops::Range;
fn invalid(detail: &str) -> RebuildError {
    RebuildError {
        code: RebuildErrorCode::UnsupportedMetadata,
        detail: detail.into(),
    }
}
#[allow(dead_code)]
pub(super) fn parse_table(
    bytes: &[u8],
    rela: bool,
    symbol_count: usize,
    symbol_sizes: &[u64],
) -> Result<Vec<Range<u64>>, RebuildError> {
    let entry = if rela { 24 } else { 16 };
    if !bytes.len().is_multiple_of(entry) || symbol_sizes.len() != symbol_count {
        return Err(invalid("Malformed dynamic relocation table"));
    }
    let mut ranges = Vec::with_capacity(bytes.len() / entry);
    for rec in bytes.chunks_exact(entry) {
        let address = u64::from_le_bytes(
            rec[..8]
                .try_into()
                .map_err(|_| invalid("Truncated dynamic relocation"))?,
        );
        let info = u64::from_le_bytes(
            rec[8..16]
                .try_into()
                .map_err(|_| invalid("Truncated dynamic relocation"))?,
        );
        let symbol = (info >> 32) as usize;
        let kind = info as u32;
        if symbol >= symbol_count {
            return Err(invalid("Dynamic relocation symbol index outside table"));
        }
        let width = match kind {
            0 => continue,
            5 => {
                let n = symbol_sizes[symbol];
                if n == 0 {
                    return Err(invalid("Copy relocation has unknown size"));
                }
                n
            }
            2 => 4,
            1 | 6 | 7 | 8 | 16 | 17 | 18 | 37 | 38 => 8,
            36 => 16,
            _ => return Err(invalid("Unsupported original dynamic relocation kind")),
        };
        let end = address
            .checked_add(width)
            .ok_or_else(|| invalid("Dynamic relocation target overflows"))?;
        ranges.push(address..end);
    }
    Ok(ranges)
}

#[derive(Default)]
struct Tags {
    rela: Option<u64>,
    relasz: Option<u64>,
    relaent: Option<u64>,
    rel: Option<u64>,
    relsz: Option<u64>,
    relent: Option<u64>,
    jmprel: Option<u64>,
    pltrelsz: Option<u64>,
    pltrel: Option<u64>,
    symtab: Option<u64>,
    syment: Option<u64>,
    strtab: Option<u64>,
    strsz: Option<u64>,
}
fn read<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], RebuildError> {
    bytes
        .get(
            at..at
                .checked_add(N)
                .ok_or_else(|| invalid("ELF offset overflow"))?,
        )
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| invalid("Truncated ELF metadata"))
}
fn word(bytes: &[u8], at: usize) -> Result<u32, RebuildError> {
    Ok(u32::from_le_bytes(read(bytes, at)?))
}
fn quad(bytes: &[u8], at: usize) -> Result<u64, RebuildError> {
    Ok(u64::from_le_bytes(read(bytes, at)?))
}
fn sub(bytes: &[u8], at: u64, len: u64) -> Result<&[u8], RebuildError> {
    let at = usize::try_from(at).map_err(|_| invalid("ELF metadata offset overflow"))?;
    let len = usize::try_from(len).map_err(|_| invalid("ELF metadata size overflow"))?;
    bytes
        .get(
            at..at
                .checked_add(len)
                .ok_or_else(|| invalid("ELF metadata range overflow"))?,
        )
        .ok_or_else(|| invalid("ELF metadata outside image"))
}
fn set(slot: &mut Option<u64>, value: u64) -> Result<(), RebuildError> {
    if slot.replace(value).is_some() {
        Err(invalid("Duplicate dynamic table tag"))
    } else {
        Ok(())
    }
}
#[derive(Clone, Copy)]
struct Table {
    address: u64,
    size: u64,
    kind: u32,
}
#[allow(dead_code)]
pub(super) fn inspect_dynamic(
    bytes: &[u8],
    headers: &[super::elf::ProgramHeader],
    shoff: usize,
    shnum: usize,
    max_symbols: usize,
) -> Result<Vec<Range<u64>>, RebuildError> {
    let dynamic: Vec<_> = headers.iter().filter(|h| h.kind == 2).collect();
    if dynamic.len() > 1 {
        return Err(invalid("Multiple dynamic segments are unsupported"));
    }
    let mut dynsym = None;
    for i in 0..shnum {
        let o = shoff
            .checked_add(
                i.checked_mul(64)
                    .ok_or_else(|| invalid("Section count overflow"))?,
            )
            .ok_or_else(|| invalid("Section offset overflow"))?;
        if word(bytes, o + 4)? == 19 {
            return Err(invalid("Packed relative relocations are unsupported"));
        }
        if word(bytes, o + 4)? == 11 {
            if dynsym.is_some() {
                return Err(invalid("Multiple dynamic symbol tables are unsupported"));
            }
            let size = quad(bytes, o + 32)?;
            let entsize = quad(bytes, o + 56)?;
            if entsize != 24 || size % 24 != 0 || size / 24 > max_symbols as u64 {
                return Err(invalid("Invalid dynamic symbol table"));
            }
            let address = quad(bytes, o + 16)?;
            let offset = quad(bytes, o + 24)?;
            let end = address
                .checked_add(size)
                .ok_or_else(|| invalid("Dynamic symbol range overflow"))?;
            if !headers.iter().any(|h| {
                h.kind == 1
                    && address >= h.address
                    && h.address.checked_add(h.file_size).is_some_and(|e| end <= e)
                    && h.offset.checked_add(address - h.address) == Some(offset)
            }) {
                return Err(invalid("Dynamic symbols are not consistently file mapped"));
            }
            let str_index = word(bytes, o + 40)? as usize;
            if str_index >= shnum {
                return Err(invalid("Dynamic string section index outside table"));
            }
            let so = shoff + str_index * 64;
            if word(bytes, so + 4)? != 3 {
                return Err(invalid("Dynamic symbols lack a string table"));
            }
            let str_address = quad(bytes, so + 16)?;
            let str_offset = quad(bytes, so + 24)?;
            let str_size = quad(bytes, so + 32)?;
            sub(bytes, str_offset, str_size)?;
            let str_end = str_address
                .checked_add(str_size)
                .ok_or_else(|| invalid("Dynamic string range overflow"))?;
            if !headers.iter().any(|h| {
                h.kind == 1
                    && str_address >= h.address
                    && h.address
                        .checked_add(h.file_size)
                        .is_some_and(|e| str_end <= e)
                    && h.offset.checked_add(str_address - h.address) == Some(str_offset)
            }) {
                return Err(invalid("Dynamic strings are not consistently file mapped"));
            }
            dynsym = Some((i, sub(bytes, offset, size)?, address, str_address, str_size));
        }
    }
    let (symbol_index, symbols, symbol_address, string_address, string_size) =
        dynsym.unwrap_or((usize::MAX, &[], 0, 0, 0));
    let symbol_count = symbols.len() / 24;
    let mut sizes = Vec::with_capacity(symbol_count);
    for s in symbols.as_chunks::<24>().0.iter() {
        sizes.push(u64::from_le_bytes(
            s[16..24]
                .try_into()
                .map_err(|_| invalid("Truncated dynamic symbol"))?,
        ));
    }
    let mut tables = Vec::new();
    let mut ranges = Vec::new();
    let mut total = 0usize;
    for i in 0..shnum {
        let o = shoff + i * 64;
        let kind = word(bytes, o + 4)?;
        if !matches!(kind, 4 | 9) || word(bytes, o + 40)? as usize != symbol_index {
            continue;
        }
        let address = quad(bytes, o + 16)?;
        let offset = quad(bytes, o + 24)?;
        let size = quad(bytes, o + 32)?;
        let entsize = quad(bytes, o + 56)?;
        let expected = if kind == 4 { 24 } else { 16 };
        if entsize != expected || size % expected != 0 {
            return Err(invalid("Malformed dynamic relocation section"));
        }
        let contents = sub(bytes, offset, size)?;
        total = total
            .checked_add(contents.len())
            .ok_or_else(|| invalid("Dynamic relocation size overflow"))?;
        if total > bytes.len() {
            return Err(invalid("Dynamic relocation sections exceed image size"));
        }
        let end = address
            .checked_add(size)
            .ok_or_else(|| invalid("Dynamic relocation address overflow"))?;
        if size != 0
            && !headers.iter().any(|h| {
                h.kind == 1
                    && address >= h.address
                    && h.address.checked_add(h.file_size).is_some_and(|e| end <= e)
                    && h.offset.checked_add(address - h.address) == Some(offset)
            })
        {
            return Err(invalid(
                "Dynamic relocation section is not consistently file mapped",
            ));
        }
        tables.push(Table {
            address,
            size,
            kind,
        });
        ranges.extend(parse_table(contents, kind == 4, symbol_count, &sizes)?);
    }
    let Some(segment) = dynamic.first() else {
        if !tables.is_empty() {
            return Err(invalid(
                "Dynamic relocation sections lack a dynamic segment",
            ));
        }
        return Ok(ranges);
    };
    let dynamic_end = segment
        .address
        .checked_add(segment.file_size)
        .ok_or_else(|| invalid("Dynamic segment range overflow"))?;
    if segment.file_size > segment.memory_size
        || !headers.iter().any(|h| {
            h.kind == 1
                && segment.address >= h.address
                && h.address
                    .checked_add(h.file_size)
                    .is_some_and(|e| dynamic_end <= e)
                && h.offset.checked_add(segment.address - h.address) == Some(segment.offset)
        })
    {
        return Err(invalid("Dynamic segment is not consistently file mapped"));
    }
    let data = sub(bytes, segment.offset, segment.file_size)?;
    if data.len() % 16 != 0 {
        return Err(invalid("Malformed dynamic segment"));
    }
    let mut tags = Tags::default();
    let mut terminated = false;
    for d in data.as_chunks::<16>().0.iter() {
        let tag = u64::from_le_bytes(
            d[..8]
                .try_into()
                .map_err(|_| invalid("Malformed dynamic tag"))?,
        );
        let value = u64::from_le_bytes(
            d[8..]
                .try_into()
                .map_err(|_| invalid("Malformed dynamic tag"))?,
        );
        match tag {
            0 => {
                terminated = true;
                break;
            }
            5 => set(&mut tags.strtab, value)?,
            6 => set(&mut tags.symtab, value)?,
            10 => set(&mut tags.strsz, value)?,
            11 => set(&mut tags.syment, value)?,
            7 => set(&mut tags.rela, value)?,
            8 => set(&mut tags.relasz, value)?,
            9 => set(&mut tags.relaent, value)?,
            17 => set(&mut tags.rel, value)?,
            18 => set(&mut tags.relsz, value)?,
            19 => set(&mut tags.relent, value)?,
            23 => set(&mut tags.jmprel, value)?,
            2 => set(&mut tags.pltrelsz, value)?,
            20 => set(&mut tags.pltrel, value)?,
            35..=37 => return Err(invalid("Packed relative relocations are unsupported")),
            22 => return Err(invalid("Text relocations are unsupported")),
            30 if value & 4 != 0 => return Err(invalid("Text relocations are unsupported")),
            _ => {}
        }
    }
    if !terminated {
        return Err(invalid("Dynamic segment has no terminator"));
    }
    if symbol_index == usize::MAX
        || tags.symtab != Some(symbol_address)
        || tags.syment != Some(24)
        || tags.strtab != Some(string_address)
        || tags.strsz != Some(string_size)
    {
        return Err(invalid(
            "Dynamic symbol/string tables do not match their mapped sections",
        ));
    }
    let matches_table =
        |address: Option<u64>, size: Option<u64>, kind: u32| -> Result<(), RebuildError> {
            match (address, size) {
                (None, None) | (None, Some(0)) => Ok(()),
                (Some(_), Some(0)) => Ok(()),
                (Some(a), Some(s))
                    if tables
                        .iter()
                        .any(|t| t.address == a && t.size == s && t.kind == kind) =>
                {
                    Ok(())
                }
                _ => Err(invalid(
                    "Dynamic relocation table does not match a validated section",
                )),
            }
        };
    if tags.rela.is_some() && tags.relaent != Some(24) {
        return Err(invalid("Invalid DT_RELAENT"));
    }
    if tags.rel.is_some() && tags.relent != Some(16) {
        return Err(invalid("Invalid DT_RELENT"));
    }
    matches_table(tags.rela, tags.relasz, 4)?;
    matches_table(tags.rel, tags.relsz, 9)?;
    match tags.pltrel {
        Some(7) => matches_table(tags.jmprel, tags.pltrelsz, 4)?,
        Some(17) => matches_table(tags.jmprel, tags.pltrelsz, 9)?,
        None if tags.jmprel.is_none() && tags.pltrelsz.unwrap_or(0) == 0 => {}
        _ => return Err(invalid("Invalid PLT relocation metadata")),
    }
    ranges.sort_unstable_by_key(|r| r.start);
    let mut merged: Vec<Range<u64>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    Ok(merged)
}
