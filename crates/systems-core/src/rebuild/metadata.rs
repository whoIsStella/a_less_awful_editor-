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
