//! Checked, borrowed memory snapshots. No filesystem or process access.
use crate::{Architecture, BinaryFormat, BinaryImage};

#[derive(Debug)]
pub struct MappedRegion<'a> {
    pub address: u64,
    pub file_offset: usize,
    pub bytes: &'a [u8],
    pub readonly: bool,
}

impl BinaryImage {
    /// Strict first native backend contract: ELF64 little-endian x86-64,
    /// executable/shared object, System V/Linux ABI. Function-specific calling
    /// convention overrides cannot be inferred from ELF headers.
    pub fn decompiler_regions(&self) -> Result<Vec<MappedRegion<'_>>, String> {
        let bytes = self.bytes();
        if self.format != BinaryFormat::Elf
            || self.architecture != Architecture::X86_64
            || bytes.get(4..7) != Some(&[2, 1, 1])
            || !matches!(bytes.get(7), Some(0 | 3))
            || bytes.get(8) != Some(&0)
            || !matches!(bytes.get(16..18), Some([2 | 3, 0]))
            || bytes.get(18..20) != Some(&[62, 0])
        {
            return Err("Native decompilation currently requires a System V/Linux ELF64 little-endian x86-64 executable or shared object".into());
        }
        self.mapped_regions(16 * 1024 * 1024, 256)
    }

    /// Returns exact file-backed bytes, never invented BSS contents. Rejects
    /// overlapping virtual ranges and file aliases rather than choosing one.
    pub fn mapped_regions(
        &self,
        max_bytes: usize,
        max_regions: usize,
    ) -> Result<Vec<MappedRegion<'_>>, String> {
        if self.mappings.is_empty() || self.mappings.len() > max_regions {
            return Err("Mapped snapshot is empty or exceeds its region limit".into());
        }
        let mut total = 0usize;
        let mut regions = Vec::with_capacity(self.mappings.len());
        for mapping in &self.mappings {
            total = total
                .checked_add(mapping.size)
                .ok_or("Mapped snapshot size overflow")?;
            if total > max_bytes {
                return Err("Mapped snapshot exceeds its byte limit".into());
            }
            mapping
                .address
                .checked_add(mapping.size as u64)
                .ok_or("Mapped address overflow")?;
            let end = mapping
                .offset
                .checked_add(mapping.size)
                .ok_or("Mapped offset overflow")?;
            let bytes = self
                .bytes()
                .get(mapping.offset..end)
                .ok_or("Mapped bytes are outside snapshot")?;
            regions.push(MappedRegion {
                address: mapping.address,
                file_offset: mapping.offset,
                bytes,
                readonly: mapping.readonly,
            });
        }
        regions.sort_by_key(|region| region.file_offset);
        if regions
            .windows(2)
            .any(|pair| pair[0].file_offset + pair[0].bytes.len() > pair[1].file_offset)
        {
            return Err("Aliased file ranges cannot be decompiled unambiguously".into());
        }
        regions.sort_by_key(|region| region.address);
        if regions
            .windows(2)
            .any(|pair| pair[0].address + pair[0].bytes.len() as u64 > pair[1].address)
        {
            return Err("Overlapping virtual ranges cannot be decompiled unambiguously".into());
        }
        Ok(regions)
    }
}
