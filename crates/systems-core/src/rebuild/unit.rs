//! Bounded, owned representation of an x86-64 relocatable object.
use super::{RebuildError, RebuildErrorCode, RebuildLimits};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UnitSectionKind {
    Text,
    ReadOnlyData,
}
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(super) struct UnitSection {
    pub(super) index: u32,
    pub(super) name: String,
    pub(super) kind: UnitSectionKind,
    pub(super) alignment: u64,
    pub(super) bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(super) struct UnitSymbol {
    pub(super) index: u32,
    pub(super) name: String,
    pub(super) section: Option<u32>,
    pub(super) offset: u64,
    pub(super) size: u64,
    pub(super) raw_binding: u8,
    pub(super) raw_type: u8,
    pub(super) raw_visibility: u8,
}
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(super) struct UnitRelocation {
    pub(super) section: u32,
    pub(super) offset: u64,
    pub(super) raw_kind: u32,
    pub(super) symbol: u32,
    pub(super) addend: i64,
}
#[derive(Debug)]
#[allow(dead_code)]
pub struct RelocatableUnit {
    pub(super) raw: Vec<u8>,
    pub(super) sections: Vec<UnitSection>,
    pub(super) symbols: Vec<UnitSymbol>,
    pub(super) relocations: Vec<UnitRelocation>,
}
#[derive(Clone, Copy)]
struct Section {
    name: u32,
    kind: u32,
    flags: u64,
    offset: u64,
    size: u64,
    link: u32,
    info: u32,
    align: u64,
    entsize: u64,
}
fn err(code: RebuildErrorCode, detail: &str) -> RebuildError {
    RebuildError {
        code,
        detail: detail.into(),
    }
}
fn cancelled(cancel: &AtomicBool) -> Result<(), RebuildError> {
    if cancel.load(Ordering::Relaxed) {
        Err(err(RebuildErrorCode::Cancelled, "Object parsing cancelled"))
    } else {
        Ok(())
    }
}
fn slice(b: &[u8], offset: u64, len: u64) -> Result<&[u8], RebuildError> {
    let start = usize::try_from(offset).map_err(|_| {
        err(
            RebuildErrorCode::UnsupportedObject,
            "Object offset overflows",
        )
    })?;
    let size = usize::try_from(len)
        .map_err(|_| err(RebuildErrorCode::UnsupportedObject, "Object size overflows"))?;
    b.get(
        start..start.checked_add(size).ok_or_else(|| {
            err(
                RebuildErrorCode::UnsupportedObject,
                "Object range overflows",
            )
        })?,
    )
    .ok_or_else(|| {
        err(
            RebuildErrorCode::UnsupportedObject,
            "Object range outside file",
        )
    })
}
fn n<const N: usize>(b: &[u8], offset: usize) -> Result<[u8; N], RebuildError> {
    b.get(
        offset..offset.checked_add(N).ok_or_else(|| {
            err(
                RebuildErrorCode::UnsupportedObject,
                "Object offset overflows",
            )
        })?,
    )
    .and_then(|v| v.try_into().ok())
    .ok_or_else(|| err(RebuildErrorCode::UnsupportedObject, "Truncated object"))
}
fn u16le(b: &[u8], o: usize) -> Result<u16, RebuildError> {
    Ok(u16::from_le_bytes(n(b, o)?))
}
fn u32le(b: &[u8], o: usize) -> Result<u32, RebuildError> {
    Ok(u32::from_le_bytes(n(b, o)?))
}
fn u64le(b: &[u8], o: usize) -> Result<u64, RebuildError> {
    Ok(u64::from_le_bytes(n(b, o)?))
}
fn string(table: &[u8], off: u32) -> Result<String, RebuildError> {
    let tail = table.get(off as usize..).ok_or_else(|| {
        err(
            RebuildErrorCode::UnsupportedObject,
            "String index outside table",
        )
    })?;
    let end = tail.iter().position(|&b| b == 0).ok_or_else(|| {
        err(
            RebuildErrorCode::UnsupportedObject,
            "Unterminated object string",
        )
    })?;
    let text = std::str::from_utf8(&tail[..end]).map_err(|_| {
        err(
            RebuildErrorCode::UnsupportedObject,
            "Object string is not UTF-8",
        )
    })?;
    if text.len() > 4096 {
        return Err(err(RebuildErrorCode::TooLarge, "Object string is too long"));
    }
    Ok(text.to_owned())
}
impl RelocatableUnit {
    pub fn parse(
        bytes: &[u8],
        function_name: &str,
        limits: &RebuildLimits,
        cancel: &AtomicBool,
    ) -> Result<Self, RebuildError> {
        cancelled(cancel)?;
        if bytes.len() > limits.max_object_bytes {
            return Err(err(RebuildErrorCode::TooLarge, "Object exceeds profile"));
        }
        if bytes.get(..7) != Some(b"\x7fELF\x02\x01\x01")
            || u16le(bytes, 16)? != 1
            || u16le(bytes, 18)? != 62
            || u32le(bytes, 20)? != 1
            || u16le(bytes, 52)? != 64
            || u16le(bytes, 58)? != 64
        {
            return Err(err(
                RebuildErrorCode::UnsupportedObject,
                "Expected ELF64 little-endian x86-64 relocatable object",
            ));
        }
        let count = usize::from(u16le(bytes, 60)?);
        let names_index = usize::from(u16le(bytes, 62)?);
        if count == 0 || names_index == 0xffff || names_index >= count {
            return Err(err(
                RebuildErrorCode::UnsupportedObject,
                "Invalid or extended section numbering",
            ));
        }
        if count > limits.max_object_sections {
            return Err(err(RebuildErrorCode::TooLarge, "Too many object sections"));
        }
        let table = slice(
            bytes,
            u64le(bytes, 40)?,
            (count as u64).checked_mul(64).ok_or_else(|| {
                err(
                    RebuildErrorCode::UnsupportedObject,
                    "Section count overflows",
                )
            })?,
        )?;
        let mut sections = Vec::with_capacity(count);
        for i in 0..count {
            cancelled(cancel)?;
            let o = i * 64;
            sections.push(Section {
                name: u32le(table, o)?,
                kind: u32le(table, o + 4)?,
                flags: u64le(table, o + 8)?,
                offset: u64le(table, o + 24)?,
                size: u64le(table, o + 32)?,
                link: u32le(table, o + 40)?,
                info: u32le(table, o + 44)?,
                align: u64le(table, o + 48)?,
                entsize: u64le(table, o + 56)?,
            });
        }
        if sections[names_index].kind != 3 {
            return Err(err(
                RebuildErrorCode::UnsupportedObject,
                "Invalid section name table",
            ));
        }
        let names = slice(
            bytes,
            sections[names_index].offset,
            sections[names_index].size,
        )?;
        let mut selected = Vec::new();
        let mut text_index = None;
        let mut rodata_size = 0usize;
        for (i, s) in sections.iter().enumerate() {
            cancelled(cancel)?;
            if s.kind == 18 {
                return Err(err(
                    RebuildErrorCode::UnsupportedObject,
                    "Extended symbol indices are unsupported",
                ));
            }
            if s.kind != 8 {
                slice(bytes, s.offset, s.size)?;
            }
            if s.align > 1 && !s.align.is_power_of_two() {
                return Err(err(
                    RebuildErrorCode::UnsupportedObject,
                    "Invalid section alignment",
                ));
            }
            if s.flags & 2 == 0 || s.size == 0 {
                continue;
            }
            if s.flags & !0x36 != 0 || s.kind != 1 {
                return Err(err(
                    RebuildErrorCode::UnsupportedObject,
                    "Writable, TLS or unsupported allocated section",
                ));
            }
            let kind = if s.flags & 4 != 0 {
                if text_index.replace(i).is_some() {
                    return Err(err(
                        RebuildErrorCode::UnsupportedObject,
                        "More than one text section",
                    ));
                }
                if s.size > limits.max_text_bytes as u64 {
                    return Err(err(RebuildErrorCode::TooLarge, "Text exceeds profile"));
                }
                UnitSectionKind::Text
            } else {
                rodata_size = rodata_size
                    .checked_add(usize::try_from(s.size).map_err(|_| {
                        err(RebuildErrorCode::TooLarge, "Read-only data exceeds profile")
                    })?)
                    .ok_or_else(|| err(RebuildErrorCode::TooLarge, "Read-only size overflow"))?;
                if rodata_size > limits.max_rodata_bytes {
                    return Err(err(
                        RebuildErrorCode::TooLarge,
                        "Read-only data exceeds profile",
                    ));
                }
                UnitSectionKind::ReadOnlyData
            };
            selected.push((i, kind));
        }
        let text_index = text_index.ok_or_else(|| {
            err(
                RebuildErrorCode::UnsupportedObject,
                "No function text section",
            )
        })?;
        let symtabs: Vec<_> = sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind == 2)
            .collect();
        if symtabs.len() != 1 {
            return Err(err(
                RebuildErrorCode::UnsupportedObject,
                "Expected one symbol table",
            ));
        }
        let (symtab_index, symtab) = symtabs[0];
        if symtab.entsize != 24 || symtab.size % 24 != 0 {
            return Err(err(
                RebuildErrorCode::UnsupportedObject,
                "Invalid symbol table",
            ));
        }
        let num = usize::try_from(symtab.size / 24)
            .map_err(|_| err(RebuildErrorCode::TooLarge, "Symbol count overflow"))?;
        if num > limits.max_symbols {
            return Err(err(RebuildErrorCode::TooLarge, "Too many object symbols"));
        }
        let strings = sections
            .get(symtab.link as usize)
            .filter(|s| s.kind == 3)
            .ok_or_else(|| {
                err(
                    RebuildErrorCode::UnsupportedObject,
                    "Invalid symbol string table",
                )
            })?;
        let strings = slice(bytes, strings.offset, strings.size)?;
        let table = slice(bytes, symtab.offset, symtab.size)?;
        let relocations = read_relocations(bytes, &sections, symtab_index, num, limits, cancel)?;
        let mut symbols = Vec::with_capacity(num);
        let mut found = 0;
        for i in 0..num {
            cancelled(cancel)?;
            let o = i * 24;
            let name = string(strings, u32le(table, o)?)?;
            let info = table[o + 4];
            let other = table[o + 5];
            let shndx = u16le(table, o + 6)?;
            let value = u64le(table, o + 8)?;
            let size = u64le(table, o + 16)?;
            let typ = info & 15;
            if info >> 4 > 2 || other & !3 != 0 || (typ == 3 && info >> 4 != 0) {
                return Err(err(
                    RebuildErrorCode::UnsupportedObject,
                    "Unsupported symbol attributes",
                ));
            }
            let file_symbol = typ == 4 && shndx == 0xfff1 && info >> 4 == 0;
            if !matches!(typ, 0..=4) || (shndx >= 0xff00 && !file_symbol) {
                return Err(err(
                    RebuildErrorCode::UnsupportedObject,
                    "TLS, IFUNC or special symbol section",
                ));
            }
            if shndx != 0 && !file_symbol && usize::from(shndx) >= count {
                return Err(err(
                    RebuildErrorCode::UnsupportedObject,
                    "Symbol section index outside table",
                ));
            }
            if shndx != 0 && !file_symbol {
                let section = sections[usize::from(shndx)];
                if value.checked_add(size).is_none_or(|end| end > section.size) {
                    return Err(err(
                        RebuildErrorCode::UnsupportedObject,
                        "Symbol extends outside its section",
                    ));
                }
                if typ == 2 && usize::from(shndx) == text_index && name != function_name {
                    return Err(err(
                        RebuildErrorCode::UnsupportedObject,
                        "Extra function definition in text section",
                    ));
                }
            }
            if name == function_name && shndx != 0 && !file_symbol {
                found += 1;
                if typ != 2
                    || usize::from(shndx) != text_index
                    || value != 0
                    || size != sections[text_index].size
                {
                    return Err(err(
                        RebuildErrorCode::UnsupportedObject,
                        "Function must occupy the complete text section",
                    ));
                }
            }
            symbols.push(UnitSymbol {
                index: i as u32,
                name,
                section: (shndx != 0 && !file_symbol).then_some(u32::from(shndx)),
                offset: value,
                size,
                raw_binding: info >> 4,
                raw_type: typ,
                raw_visibility: other & 3,
            });
        }
        if found != 1 {
            return Err(err(
                RebuildErrorCode::UnsupportedObject,
                "Expected exactly one named function definition",
            ));
        }
        let mut owned_sections = Vec::with_capacity(selected.len());
        for (i, kind) in selected {
            cancelled(cancel)?;
            let s = sections[i];
            owned_sections.push(UnitSection {
                index: i as u32,
                name: string(names, s.name)?,
                kind,
                alignment: s.align.max(1),
                bytes: slice(bytes, s.offset, s.size)?.to_vec(),
            });
        }
        Ok(Self {
            raw: bytes.to_vec(),
            sections: owned_sections,
            symbols,
            relocations,
        })
    }
}
fn read_relocations(
    bytes: &[u8],
    sections: &[Section],
    symtab_index: usize,
    symbol_count: usize,
    limits: &RebuildLimits,
    cancel: &AtomicBool,
) -> Result<Vec<UnitRelocation>, RebuildError> {
    let mut total = 0usize;
    for s in sections {
        if s.kind == 9 && s.size != 0 {
            return Err(err(
                RebuildErrorCode::UnsupportedRelocation,
                "REL implicit addends are unsupported",
            ));
        }
        if s.kind == 4 {
            if s.entsize != 24
                || s.size % 24 != 0
                || s.link as usize != symtab_index
                || s.info as usize >= sections.len()
            {
                return Err(err(
                    RebuildErrorCode::UnsupportedRelocation,
                    "Invalid RELA table metadata",
                ));
            }
            let count = usize::try_from(s.size / 24)
                .map_err(|_| err(RebuildErrorCode::TooLarge, "Relocation count overflows"))?;
            total = total
                .checked_add(count)
                .ok_or_else(|| err(RebuildErrorCode::TooLarge, "Relocation count overflows"))?;
            if total > limits.max_relocations {
                return Err(err(RebuildErrorCode::TooLarge, "Too many relocations"));
            }
        }
    }
    let mut relocations = Vec::with_capacity(total);
    let mut fields = Vec::with_capacity(total);
    for s in sections.iter().filter(|s| s.kind == 4) {
        cancelled(cancel)?;
        let target = &sections[s.info as usize];
        let data = slice(bytes, s.offset, s.size)?;
        for i in 0..(s.size / 24) as usize {
            cancelled(cancel)?;
            let o = i * 24;
            let offset = u64le(data, o)?;
            let info = u64le(data, o + 8)?;
            let raw_kind = info as u32;
            let symbol = (info >> 32) as u32;
            let addend = i64::from_le_bytes(n(data, o + 16)?);
            if symbol as usize >= symbol_count {
                return Err(err(
                    RebuildErrorCode::UnsupportedRelocation,
                    "Relocation symbol index outside table",
                ));
            }
            if !matches!(raw_kind, 0 | 2 | 4) {
                return Err(err(
                    RebuildErrorCode::UnsupportedRelocation,
                    "Unsupported x86-64 relocation kind",
                ));
            }
            if raw_kind != 0 {
                let end = offset.checked_add(4).ok_or_else(|| {
                    err(
                        RebuildErrorCode::UnsupportedRelocation,
                        "Relocation field overflow",
                    )
                })?;
                if end > target.size {
                    return Err(err(
                        RebuildErrorCode::UnsupportedRelocation,
                        "Relocation field outside section",
                    ));
                }
                fields.push((s.info, offset, end));
            }
            if target.flags & 2 != 0 && target.size != 0 {
                relocations.push(UnitRelocation {
                    section: s.info,
                    offset,
                    raw_kind,
                    symbol,
                    addend,
                });
            }
        }
    }
    fields.sort_unstable();
    if fields
        .windows(2)
        .any(|pair| pair[0].0 == pair[1].0 && pair[0].2 > pair[1].1)
    {
        return Err(err(RebuildErrorCode::Overlap, "Relocation fields overlap"));
    }
    Ok(relocations)
}
