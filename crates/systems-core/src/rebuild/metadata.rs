//! Versioned on-disk location metadata for a single rewritten function.
use super::{RebuildError, RebuildErrorCode};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewriteLocation {
    pub entry_address: u64,
    pub original_capacity: u64,
    pub body_address: u64,
    pub body_size: u64,
}
#[allow(dead_code)]
pub(super) fn encode_note(location: &RewriteLocation) -> Vec<u8> {
    let mut note = Vec::with_capacity(52);
    note.extend_from_slice(&4u32.to_le_bytes());
    note.extend_from_slice(&36u32.to_le_bytes());
    note.extend_from_slice(&1u32.to_le_bytes());
    note.extend_from_slice(b"ALE\0");
    note.extend_from_slice(&1u32.to_le_bytes());
    for value in [
        location.entry_address,
        location.original_capacity,
        location.body_address,
        location.body_size,
    ] {
        note.extend_from_slice(&value.to_le_bytes());
    }
    note
}
#[allow(dead_code)]
pub(super) fn decode_note(note: &[u8]) -> Result<RewriteLocation, RebuildError> {
    let bad = || RebuildError {
        code: RebuildErrorCode::UnsupportedMetadata,
        detail: "Malformed or unsupported rewrite note".into(),
    };
    if note.len() != 52
        || note.get(..16) != Some(&[4, 0, 0, 0, 36, 0, 0, 0, 1, 0, 0, 0, b'A', b'L', b'E', 0])
        || note.get(16..20) != Some(&1u32.to_le_bytes())
    {
        return Err(bad());
    }
    let read = |offset: usize| -> Result<u64, RebuildError> {
        Ok(u64::from_le_bytes(
            note.get(offset..offset + 8)
                .and_then(|v| v.try_into().ok())
                .ok_or_else(bad)?,
        ))
    };
    Ok(RewriteLocation {
        entry_address: read(20)?,
        original_capacity: read(28)?,
        body_address: read(36)?,
        body_size: read(44)?,
    })
}

pub(crate) fn read_rewrite_location(
    image: &crate::BinaryImage,
) -> Result<Option<RewriteLocation>, String> {
    use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol};
    if image.format != crate::BinaryFormat::Elf {
        return Ok(None);
    }
    let file = object::File::parse(image.bytes()).map_err(|e| e.to_string())?;
    let mut notes = file
        .sections()
        .filter(|s| s.name().ok() == Some(".note.ale.rewrite"));
    let Some(note) = notes.next() else {
        return Ok(None);
    };
    if notes.next().is_some()
        || note.kind() != object::SectionKind::Note
        || note.align() != 4
        || !matches!(note.flags(), object::SectionFlags::Elf { sh_flags: 0 })
    {
        return Err("ambiguous or invalid note section".into());
    }
    let loc = decode_note(note.data().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let site = crate::recompile::ReplacementSite::discover(image, loc.entry_address)?;
    if site.capacity as u64 != loc.original_capacity || site.capacity < 5 || loc.body_size == 0 {
        return Err("entry or body size mismatch".into());
    }
    let entry_end = loc
        .entry_address
        .checked_add(loc.original_capacity)
        .ok_or("entry range overflow")?;
    let body_end = loc
        .body_address
        .checked_add(loc.body_size)
        .ok_or("body range overflow")?;
    let bytes = image
        .bytes()
        .get(
            site.offset
                ..site
                    .offset
                    .checked_add(site.capacity)
                    .ok_or("entry offset overflow")?,
        )
        .ok_or("entry outside image")?;
    if bytes[0] != 0xe9 || bytes[5..].iter().any(|&x| x != 0x90) {
        return Err("entry is not a canonical jump stub".into());
    }
    let disp = i32::from_le_bytes(bytes[1..5].try_into().map_err(|_| "invalid jump")?);
    let target = i128::from(loc.entry_address) + 5 + i128::from(disp);
    if u64::try_from(target).ok() != Some(loc.body_address)
        || (loc.entry_address < body_end && loc.body_address < entry_end)
    {
        return Err("entry target does not match body".into());
    }
    let body_name = format!("__ale_body_{}", site.name);
    let mut symbols = file
        .symbols()
        .filter(|s| s.name().ok() == Some(body_name.as_str()));
    let symbol = symbols.next().ok_or("missing body symbol")?;
    if symbols.next().is_some()
        || symbol.kind() != object::SymbolKind::Text
        || symbol.address() != loc.body_address
        || symbol.size() != loc.body_size
    {
        return Err("body symbol mismatch".into());
    }
    let section = file
        .section_by_index(symbol.section_index().ok_or("body has no section")?)
        .map_err(|e| e.to_string())?;
    if section.name().ok() != Some(format!(".ale.text.{}", site.name).as_str())
        || section.address() != loc.body_address
        || section.size() != loc.body_size
        || !matches!(section.flags(), object::SectionFlags::Elf { sh_flags: 6 })
    {
        return Err("body section mismatch".into());
    }
    let (offset, size) = section.file_range().ok_or("body is not file backed")?;
    if size != loc.body_size
        || image.address_to_offset(loc.body_address) != usize::try_from(offset).ok()
        || image.address_to_offset(body_end - 1)
            != usize::try_from(
                offset
                    .checked_add(size - 1)
                    .ok_or("body file range overflow")?,
            )
            .ok()
    {
        return Err("body section mapping mismatch".into());
    }
    let mapped = file.segments().any(|segment| {
        let (file_offset, file_size) = segment.file_range();
        matches!(segment.flags(),object::SegmentFlags::Elf {p_flags} if p_flags==5)
            && loc.body_address >= segment.address()
            && segment
                .address()
                .checked_add(file_size)
                .is_some_and(|end| body_end <= end)
            && file_offset.checked_add(loc.body_address - segment.address()) == Some(offset)
    });
    if !mapped {
        return Err("body is not mapped by a read-execute load segment".into());
    }
    Ok(Some(loc))
}
