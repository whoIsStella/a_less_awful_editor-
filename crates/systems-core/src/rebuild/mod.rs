//! Immutable replacement plans for native images.
use crate::{BinaryImage, recompile::ReplacementSite};
use std::{fmt, ops::Range, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementMode {
    FixedSpan,
    Relocated,
}

#[derive(Clone, Copy, Debug)]
pub struct RebuildLimits {
    pub max_image_bytes: usize,
    pub max_object_bytes: usize,
    pub max_text_bytes: usize,
    pub max_rodata_bytes: usize,
    pub max_object_sections: usize,
    pub max_symbols: usize,
    pub max_relocations: usize,
    pub max_input_program_headers: usize,
    pub max_section_headers: usize,
}
impl Default for RebuildLimits {
    fn default() -> Self {
        Self {
            max_image_bytes: 64 * 1024 * 1024,
            max_object_bytes: 2 * 1024 * 1024,
            max_text_bytes: 1024 * 1024,
            max_rodata_bytes: 1024 * 1024,
            max_object_sections: 128,
            max_symbols: 4096,
            max_relocations: 16384,
            max_input_program_headers: 256,
            max_section_headers: 16384,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct RebuildOptions {
    pub mode: PlacementMode,
    pub limits: RebuildLimits,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebuildErrorCode {
    UnsupportedImage,
    InvalidLayout,
    UnsupportedObject,
    UnsupportedRelocation,
    AmbiguousSymbol,
    UndefinedSymbol,
    RelocationOverflow,
    Overlap,
    UnsupportedMetadata,
    TooLarge,
    Cancelled,
    StaleImage,
}
#[derive(Debug)]
pub struct RebuildError {
    pub code: RebuildErrorCode,
    pub detail: String,
}
impl fmt::Display for RebuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.detail)
    }
}
impl std::error::Error for RebuildError {}
#[derive(Clone, Debug)]
pub struct ResolvedRelocation {
    pub raw_kind: u32,
    pub field_address: u64,
    pub target_address: u64,
    pub addend: i64,
    pub symbol: String,
}
#[derive(Clone, Debug)]
pub struct PlanSummary {
    pub mode: PlacementMode,
    pub entry_address: u64,
    pub body_address: u64,
    pub original_span: Range<usize>,
    pub body_span: Range<usize>,
    pub rodata_bytes: usize,
    pub changed_original_ranges: Vec<Range<usize>>,
    pub appended_bytes: usize,
    pub relocations: Vec<ResolvedRelocation>,
    pub warnings: Vec<String>,
}
pub struct ReplacementPlan {
    base: Arc<[u8]>,
    candidate: BinaryImage,
    summary: PlanSummary,
}
impl ReplacementPlan {
    pub fn from_fixed(
        image: &BinaryImage,
        site: &ReplacementSite,
        bytes: Vec<u8>,
    ) -> Result<Self, RebuildError> {
        let invalid = |detail: String| RebuildError {
            code: RebuildErrorCode::InvalidLayout,
            detail,
        };
        let actual = ReplacementSite::discover(image, site.address).map_err(invalid)?;
        if actual.name != site.name
            || actual.address != site.address
            || actual.offset != site.offset
            || actual.capacity != site.capacity
        {
            return Err(invalid("Replacement site does not match the image".into()));
        }
        if bytes.len() != site.capacity {
            return Err(invalid(
                "Replacement length does not match the function span".into(),
            ));
        }
        let end = site
            .offset
            .checked_add(site.capacity)
            .ok_or_else(|| invalid("Replacement range overflows".into()))?;
        let candidate = image.patched(site.offset, &bytes).map_err(invalid)?;
        let span = site.offset..end;
        Ok(Self {
            base: Arc::clone(&image.bytes),
            candidate,
            summary: PlanSummary {
                mode: PlacementMode::FixedSpan,
                entry_address: site.address,
                body_address: site.address,
                original_span: span.clone(),
                body_span: span.clone(),
                rodata_bytes: 0,
                changed_original_ranges: vec![span],
                appended_bytes: 0,
                relocations: vec![],
                warnings: vec![],
            },
        })
    }
    pub fn summary(&self) -> &PlanSummary {
        &self.summary
    }
    pub fn candidate(&self) -> &BinaryImage {
        &self.candidate
    }
    pub fn apply_to(&self, image: &BinaryImage) -> Result<BinaryImage, RebuildError> {
        if image.bytes() != self.base.as_ref() {
            return Err(RebuildError {
                code: RebuildErrorCode::StaleImage,
                detail: "Image changed since this replacement was planned".into(),
            });
        }
        Ok(self.candidate.clone())
    }
}
#[cfg(test)]
mod tests;
