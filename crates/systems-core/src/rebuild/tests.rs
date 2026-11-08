use super::*;
use crate::{BinaryImage, recompile::ReplacementSite};

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
fn fixture() -> (BinaryImage, ReplacementSite) {
    let mut b = vec![0; 0x500];
    b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    put16(&mut b, 16, 2);
    put16(&mut b, 18, 62);
    put32(&mut b, 20, 1);
    put64(&mut b, 24, 0x401000);
    put64(&mut b, 40, 0x380);
    put16(&mut b, 52, 64);
    put16(&mut b, 58, 64);
    put16(&mut b, 60, 5);
    put16(&mut b, 62, 2);
    b[0x100..0x108].copy_from_slice(&[0xb8, 1, 0, 0, 0, 0xc3, 0x90, 0x90]);
    let names = b"\0.text\0.shstrtab\0.strtab\0.symtab\0";
    b[0x180..0x180 + names.len()].copy_from_slice(names);
    b[0x1c0..0x1c9].copy_from_slice(b"\0example\0");
    put32(&mut b, 0x218, 1);
    b[0x21c] = 0x12;
    put16(&mut b, 0x21e, 1);
    put64(&mut b, 0x220, 0x401000);
    put64(&mut b, 0x228, 8);
    let s = 0x3c0;
    put32(&mut b, s, 1);
    put32(&mut b, s + 4, 1);
    put64(&mut b, s + 8, 6);
    put64(&mut b, s + 16, 0x401000);
    put64(&mut b, s + 24, 0x100);
    put64(&mut b, s + 32, 8);
    put64(&mut b, s + 48, 1);
    let s = 0x400;
    put32(&mut b, s, 7);
    put32(&mut b, s + 4, 3);
    put64(&mut b, s + 24, 0x180);
    put64(&mut b, s + 32, names.len() as u64);
    put64(&mut b, s + 48, 1);
    let s = 0x440;
    put32(&mut b, s, 17);
    put32(&mut b, s + 4, 3);
    put64(&mut b, s + 24, 0x1c0);
    put64(&mut b, s + 32, 9);
    put64(&mut b, s + 48, 1);
    let s = 0x480;
    put32(&mut b, s, 25);
    put32(&mut b, s + 4, 2);
    put64(&mut b, s + 24, 0x200);
    put64(&mut b, s + 32, 48);
    put32(&mut b, s + 40, 3);
    put32(&mut b, s + 44, 1);
    put64(&mut b, s + 48, 8);
    put64(&mut b, s + 56, 24);
    let image = BinaryImage::parse(b).unwrap();
    let site = ReplacementSite::discover(&image, 0x401000).unwrap();
    (image, site)
}
fn replacement() -> Vec<u8> {
    vec![0xb8, 2, 0, 0, 0, 0xc3, 0x90, 0x90]
}
#[test]
fn fixed_plan_applies_only_to_exact_base() {
    let (image, site) = fixture();
    let plan = ReplacementPlan::from_fixed(&image, &site, replacement()).unwrap();
    assert_eq!(
        plan.apply_to(&image.clone()).unwrap().bytes(),
        plan.candidate().bytes()
    );
    let other = image.patched(0x300, &[1]).unwrap();
    assert_eq!(
        plan.apply_to(&other).unwrap_err().code,
        RebuildErrorCode::StaleImage
    );
}
#[test]
fn fixed_plan_has_one_declared_change() {
    let (image, site) = fixture();
    let bytes = replacement();
    let legacy = image.patched(site.offset, &bytes).unwrap();
    let plan = ReplacementPlan::from_fixed(&image, &site, bytes).unwrap();
    assert_eq!(plan.candidate().bytes(), legacy.bytes());
    assert_eq!(
        plan.summary().changed_original_ranges,
        vec![site.offset..site.offset + site.capacity]
    );
    assert_eq!(plan.summary().appended_bytes, 0);
}
#[test]
fn fixed_plan_rejects_wrong_length() {
    let (image, site) = fixture();
    assert!(ReplacementPlan::from_fixed(&image, &site, vec![0; site.capacity - 1]).is_err());
    assert!(ReplacementPlan::from_fixed(&image, &site, vec![0; site.capacity + 1]).is_err());
    let mut forged = site.clone();
    forged.offset += 1;
    assert!(ReplacementPlan::from_fixed(&image, &forged, replacement()).is_err());
}
#[test]
fn fixed_plan_does_not_change_original_image() {
    let (image, site) = fixture();
    let original = image.bytes().to_vec();
    let plan = ReplacementPlan::from_fixed(&image, &site, replacement()).unwrap();
    assert_eq!(image.bytes(), original);
    assert_ne!(plan.candidate().bytes(), original);
}
#[test]
fn limits_are_the_documented_profile() {
    let l = RebuildLimits::default();
    assert_eq!(
        (
            l.max_image_bytes,
            l.max_object_bytes,
            l.max_text_bytes,
            l.max_rodata_bytes
        ),
        (64 * 1024 * 1024, 2 * 1024 * 1024, 1024 * 1024, 1024 * 1024)
    );
    assert_eq!(
        (
            l.max_object_sections,
            l.max_symbols,
            l.max_relocations,
            l.max_input_program_headers,
            l.max_section_headers
        ),
        (128, 4096, 16384, 256, 16384)
    );
}

fn loadable_fixture() -> (BinaryImage, ReplacementSite) {
    let (image, _) = fixture();
    let mut bytes = image.bytes().to_vec();
    put64(&mut bytes, 32, 64);
    put16(&mut bytes, 54, 56);
    put16(&mut bytes, 56, 1);
    put32(&mut bytes, 64, 1);
    put32(&mut bytes, 68, 5);
    put64(&mut bytes, 72, 0x100);
    put64(&mut bytes, 80, 0x401000);
    put64(&mut bytes, 88, 0x401000);
    put64(&mut bytes, 96, 8);
    put64(&mut bytes, 104, 8);
    put64(&mut bytes, 112, 0x100);
    let image = BinaryImage::parse(bytes).unwrap();
    let site = ReplacementSite::discover(&image, 0x401000).unwrap();
    (image, site)
}
#[test]
fn admission_valid_exec_and_rejects_bad_headers_without_mutation() {
    let (image, site) = loadable_fixture();
    let limits = RebuildLimits::default();
    assert!(super::elf::inspect_image(&image, &site, &limits).is_ok());
    for (offset, value) in [
        (4, 1u8),
        (5, 2),
        (16, 3),
        (18, 183),
        (52, 63),
        (54, 55),
        (58, 63),
        (56, 255),
    ] {
        let mut candidate = image.clone();
        let mut bytes = candidate.bytes().to_vec();
        bytes[offset] = value;
        candidate.bytes = bytes.into();
        let original = candidate.bytes().to_vec();
        assert!(
            super::elf::inspect_image(&candidate, &site, &limits).is_err(),
            "offset {offset}"
        );
        assert_eq!(candidate.bytes(), original);
    }
    for (offset, value) in [(32, u64::MAX), (40, u64::MAX)] {
        let mut candidate = image.clone();
        let mut bytes = candidate.bytes().to_vec();
        put64(&mut bytes, offset, value);
        candidate.bytes = bytes.into();
        assert!(super::elf::inspect_image(&candidate, &site, &limits).is_err());
    }
    let mut truncated = image.clone();
    truncated.bytes = image.bytes()[..63].into();
    assert!(super::elf::inspect_image(&truncated, &site, &limits).is_err());
}
#[test]
fn admission_rejects_non_executable_wx_or_inconsistent_loads() {
    let (image, site) = loadable_fixture();
    for (offset, value) in [(68, 6u8), (68, 7), (104, 7), (112, 3)] {
        let mut bytes = image.bytes().to_vec();
        bytes[offset] = value;
        let candidate = BinaryImage::parse(bytes).unwrap();
        assert!(
            super::elf::inspect_image(&candidate, &site, &RebuildLimits::default()).is_err(),
            "offset {offset} value {value}"
        );
    }
}

fn simple_eh_frame(start: u64, length: u64) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend_from_slice(&9u32.to_le_bytes());
    frame.extend_from_slice(&0u32.to_le_bytes());
    frame.extend_from_slice(&[1, 0, 1, 0x78, 16]);
    frame.extend_from_slice(&20u32.to_le_bytes());
    frame.extend_from_slice(&17u32.to_le_bytes());
    frame.extend_from_slice(&start.to_le_bytes());
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(&0u32.to_le_bytes());
    frame
}
#[test]
fn admission_unwind_coverage_and_malformed_frames() {
    let check = |data: &[u8]| super::elf::check_unwind(data, 0, 0x401000..0x401008);
    assert!(check(&simple_eh_frame(0x402000, 32)).is_ok());
    assert_eq!(
        check(&simple_eh_frame(0x400fff, 2)).unwrap_err().code,
        RebuildErrorCode::UnsupportedMetadata
    );
    assert_eq!(
        check(&simple_eh_frame(0x401007, 2)).unwrap_err().code,
        RebuildErrorCode::UnsupportedMetadata
    );
    assert_eq!(
        check(&[20, 0, 0, 0, 0]).unwrap_err().code,
        RebuildErrorCode::UnsupportedMetadata
    );
    let mut bad_instruction = simple_eh_frame(0x402000, 32);
    put32(&mut bad_instruction, 13, 21);
    bad_instruction.insert(37, 0x3f);
    assert_eq!(
        check(&bad_instruction).unwrap_err().code,
        RebuildErrorCode::UnsupportedMetadata
    );
    let mut unsupported = simple_eh_frame(0x402000, 32);
    unsupported[8] = 0xff;
    assert_eq!(
        check(&unsupported).unwrap_err().code,
        RebuildErrorCode::UnsupportedMetadata
    );
}

#[test]
fn admission_gnu_property_rejects_cet_and_malformed_notes() {
    let mut note = Vec::new();
    note.extend_from_slice(&4u32.to_le_bytes());
    note.extend_from_slice(&16u32.to_le_bytes());
    note.extend_from_slice(&5u32.to_le_bytes());
    note.extend_from_slice(b"GNU\0");
    note.extend_from_slice(&0xc0000002u32.to_le_bytes());
    note.extend_from_slice(&4u32.to_le_bytes());
    note.extend_from_slice(&0u32.to_le_bytes());
    note.extend_from_slice(&0u32.to_le_bytes());
    assert!(super::elf::check_notes(&note).is_ok());
    note[24] = 1;
    assert_eq!(
        super::elf::check_notes(&note).unwrap_err().code,
        RebuildErrorCode::UnsupportedMetadata
    );
    assert_eq!(
        super::elf::check_notes(&note[..27]).unwrap_err().code,
        RebuildErrorCode::UnsupportedMetadata
    );
}

#[test]
fn admission_rejects_known_external_branch_into_function_interior() {
    let (image, _) = loadable_fixture();
    let mut bytes = image.bytes().to_vec();
    put64(&mut bytes, 96, 16);
    put64(&mut bytes, 104, 16);
    put64(&mut bytes, 0x3c0 + 32, 16);
    bytes[0x108..0x10d].copy_from_slice(&[0xe9, 0xf4, 0xff, 0xff, 0xff]);
    let candidate = BinaryImage::parse(bytes).unwrap();
    let site = ReplacementSite::discover(&candidate, 0x401000).unwrap();
    assert!(super::elf::inspect_image(&candidate, &site, &RebuildLimits::default()).is_err());
}
#[test]
fn admission_rejects_interior_data_symbol() {
    let (image, _) = loadable_fixture();
    let mut bytes = image.bytes().to_vec();
    bytes[0x1c9..0x1ce].copy_from_slice(b"data\0");
    put64(&mut bytes, 0x440 + 32, 14);
    put64(&mut bytes, 0x480 + 32, 72);
    put32(&mut bytes, 0x230, 9);
    bytes[0x234] = 0x11;
    put16(&mut bytes, 0x236, 1);
    put64(&mut bytes, 0x238, 0x401001);
    put64(&mut bytes, 0x240, 1);
    let candidate = BinaryImage::parse(bytes).unwrap();
    let site = ReplacementSite::discover(&candidate, 0x401000).unwrap();
    assert!(super::elf::inspect_image(&candidate, &site, &RebuildLimits::default()).is_err());
}

#[test]
fn admission_rejects_excess_symbol_count_and_extended_indices() {
    let (image, site) = loadable_fixture();
    let limits = RebuildLimits {
        max_symbols: 1,
        ..RebuildLimits::default()
    };
    assert_eq!(
        super::elf::inspect_image(&image, &site, &limits)
            .unwrap_err()
            .code,
        RebuildErrorCode::TooLarge
    );
    let mut bytes = image.bytes().to_vec();
    put32(&mut bytes, 0x440 + 4, 18);
    let mut candidate = image.clone();
    candidate.bytes = bytes.into();
    assert_eq!(
        super::elf::inspect_image(&candidate, &site, &RebuildLimits::default())
            .unwrap_err()
            .code,
        RebuildErrorCode::UnsupportedMetadata
    );
}

#[test]
fn admission_rejects_overlapping_or_unsorted_load_segments() {
    let (image, site) = loadable_fixture();
    let mut bytes = image.bytes().to_vec();
    put16(&mut bytes, 56, 2);
    put32(&mut bytes, 120, 1);
    put32(&mut bytes, 124, 4);
    put64(&mut bytes, 128, 0x200);
    put64(&mut bytes, 136, 0x402000);
    put64(&mut bytes, 144, 0x402000);
    put64(&mut bytes, 152, 16);
    put64(&mut bytes, 160, 16);
    put64(&mut bytes, 168, 0x100);
    let candidate = BinaryImage::parse(bytes.clone()).unwrap();
    assert!(super::elf::inspect_image(&candidate, &site, &RebuildLimits::default()).is_ok());
    let mut file_overlap = bytes.clone();
    put64(&mut file_overlap, 128, 0x100);
    let mut overlapping_file = image.clone();
    overlapping_file.bytes = file_overlap.into();
    assert!(
        super::elf::inspect_image(&overlapping_file, &site, &RebuildLimits::default()).is_err()
    );
    put64(&mut bytes, 136, 0x400000);
    let mut unsorted = image.clone();
    unsorted.bytes = bytes.clone().into();
    assert!(super::elf::inspect_image(&unsorted, &site, &RebuildLimits::default()).is_err());
    put64(&mut bytes, 136, 0x401000);
    put64(&mut bytes, 128, 0x100);
    let mut overlap = image.clone();
    overlap.bytes = bytes.into();
    assert!(super::elf::inspect_image(&overlap, &site, &RebuildLimits::default()).is_err());
}

#[test]
fn admission_rejects_rewrite_marker_and_unreadable_unwind_index() {
    let (image, site) = loadable_fixture();
    let mut bytes = image.bytes().to_vec();
    let name = b".note.ale.rewrite\0";
    bytes[0x1a1..0x1a1 + name.len()].copy_from_slice(name);
    put64(&mut bytes, 0x400 + 32, (33 + name.len()) as u64);
    put32(&mut bytes, 0x400, 33);
    let candidate = BinaryImage::parse(bytes).unwrap();
    assert_eq!(
        super::elf::inspect_image(&candidate, &site, &RebuildLimits::default())
            .unwrap_err()
            .code,
        RebuildErrorCode::UnsupportedMetadata
    );
    let mut bytes = image.bytes().to_vec();
    put16(&mut bytes, 56, 2);
    put32(&mut bytes, 120, 0x6474e550);
    let candidate = BinaryImage::parse(bytes).unwrap();
    assert_eq!(
        super::elf::inspect_image(&candidate, &site, &RebuildLimits::default())
            .unwrap_err()
            .code,
        RebuildErrorCode::UnsupportedMetadata
    );
}
#[test]
fn admission_rejects_static_symbol_index_consumers() {
    let (image, site) = loadable_fixture();
    let mut bytes = image.bytes().to_vec();
    put16(&mut bytes, 60, 6);
    put32(&mut bytes, 0x4c0 + 4, 4);
    put32(&mut bytes, 0x4c0 + 40, 4);
    let mut candidate = image.clone();
    candidate.bytes = bytes.into();
    assert_eq!(
        super::elf::inspect_image(&candidate, &site, &RebuildLimits::default())
            .unwrap_err()
            .code,
        RebuildErrorCode::UnsupportedMetadata
    );
}

#[test]
fn admission_rejects_extended_numbering_and_size_caps() {
    let (image, site) = loadable_fixture();
    for (offset, value) in [(56, 0xffff), (60, 0), (62, 0xffff)] {
        let mut candidate = image.clone();
        let mut bytes = image.bytes().to_vec();
        put16(&mut bytes, offset, value);
        candidate.bytes = bytes.into();
        assert!(super::elf::inspect_image(&candidate, &site, &RebuildLimits::default()).is_err());
    }
    for limits in [
        RebuildLimits {
            max_image_bytes: image.bytes().len() - 1,
            ..RebuildLimits::default()
        },
        RebuildLimits {
            max_input_program_headers: 0,
            ..RebuildLimits::default()
        },
        RebuildLimits {
            max_section_headers: 4,
            ..RebuildLimits::default()
        },
    ] {
        assert_eq!(
            super::elf::inspect_image(&image, &site, &limits)
                .unwrap_err()
                .code,
            RebuildErrorCode::TooLarge
        );
    }
}

#[test]
fn admission_rejects_short_entry_and_different_extent_alias() {
    let (image, _) = loadable_fixture();
    let mut bytes = image.bytes().to_vec();
    put64(&mut bytes, 0x228, 4);
    let candidate = BinaryImage::parse(bytes).unwrap();
    let site = ReplacementSite::discover(&candidate, 0x401000).unwrap();
    assert!(super::elf::inspect_image(&candidate, &site, &RebuildLimits::default()).is_err());
    let mut bytes = image.bytes().to_vec();
    bytes[0x1c9..0x1cf].copy_from_slice(b"alias\0");
    put64(&mut bytes, 0x440 + 32, 15);
    put64(&mut bytes, 0x480 + 32, 72);
    put32(&mut bytes, 0x230, 9);
    bytes[0x234] = 0x12;
    put16(&mut bytes, 0x236, 1);
    put64(&mut bytes, 0x238, 0x401000);
    put64(&mut bytes, 0x240, 7);
    let mut candidate = image.clone();
    candidate.bytes = bytes.into();
    let site = ReplacementSite::discover(&image, 0x401000).unwrap();
    assert!(super::elf::inspect_image(&candidate, &site, &RebuildLimits::default()).is_err());
}

fn object_fixture() -> Vec<u8> {
    let (image, _) = fixture();
    let mut bytes = image.bytes().to_vec();
    put16(&mut bytes, 16, 1);
    put64(&mut bytes, 24, 0);
    put64(&mut bytes, 0x3c0 + 16, 0);
    put64(&mut bytes, 0x220, 0);
    bytes
}
#[test]
fn unit_accepts_complete_function_and_preserves_raw_object() {
    use std::sync::atomic::AtomicBool;
    let bytes = object_fixture();
    let unit = RelocatableUnit::parse(
        &bytes,
        "example",
        &RebuildLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(unit.raw, bytes);
    assert_eq!(unit.sections.len(), 1);
    assert_eq!(unit.sections[0].bytes, &bytes[0x100..0x108]);
    assert_eq!(
        unit.symbols
            .iter()
            .find(|s| s.name == "example")
            .unwrap()
            .size,
        8
    );
}
#[test]
fn unit_rejects_wrong_name_format_and_limits() {
    use std::sync::atomic::AtomicBool;
    let bytes = object_fixture();
    let c = AtomicBool::new(false);
    assert!(RelocatableUnit::parse(&bytes, "wrong", &RebuildLimits::default(), &c).is_err());
    let mut wrong = bytes.clone();
    put16(&mut wrong, 16, 2);
    assert!(RelocatableUnit::parse(&wrong, "example", &RebuildLimits::default(), &c).is_err());
    let limits = RebuildLimits {
        max_object_bytes: bytes.len() - 1,
        ..RebuildLimits::default()
    };
    assert_eq!(
        RelocatableUnit::parse(&bytes, "example", &limits, &c)
            .unwrap_err()
            .code,
        RebuildErrorCode::TooLarge
    );
    assert_eq!(
        RelocatableUnit::parse(
            &bytes,
            "example",
            &RebuildLimits::default(),
            &AtomicBool::new(true)
        )
        .unwrap_err()
        .code,
        RebuildErrorCode::Cancelled
    );
}

fn relocation_object() -> Vec<u8> {
    let mut bytes = object_fixture();
    put16(&mut bytes, 60, 6);
    bytes[0x1c9..0x1cd].copy_from_slice(b"ext\0");
    put64(&mut bytes, 0x440 + 32, 13);
    put64(&mut bytes, 0x480 + 32, 72);
    put32(&mut bytes, 0x230, 9);
    bytes[0x234] = 0x10;
    put64(&mut bytes, 0x300, 1);
    put64(&mut bytes, 0x308, (2u64 << 32) | 2);
    bytes[0x310..0x318].copy_from_slice(&(-7i64).to_le_bytes());
    put32(&mut bytes, 0x4c0 + 4, 4);
    put64(&mut bytes, 0x4c0 + 24, 0x300);
    put64(&mut bytes, 0x4c0 + 32, 24);
    put32(&mut bytes, 0x4c0 + 40, 4);
    put32(&mut bytes, 0x4c0 + 44, 1);
    put64(&mut bytes, 0x4c0 + 48, 8);
    put64(&mut bytes, 0x4c0 + 56, 24);
    bytes
}
fn readonly_object() -> Vec<u8> {
    let mut bytes = relocation_object();
    bytes.resize(0x540, 0);
    put16(&mut bytes, 60, 7);
    bytes[0x1a1..0x1a9].copy_from_slice(b".rodata\0");
    put64(&mut bytes, 0x400 + 32, 41);
    bytes[0x340..0x344].copy_from_slice(&[3, 7, 11, 19]);
    put32(&mut bytes, 0x500, 33);
    put32(&mut bytes, 0x500 + 4, 1);
    put64(&mut bytes, 0x500 + 8, 2);
    put64(&mut bytes, 0x500 + 24, 0x340);
    put64(&mut bytes, 0x500 + 32, 4);
    put64(&mut bytes, 0x500 + 48, 4);
    bytes
}
#[test]
fn unit_preserves_explicit_relocation_kind_symbol_and_addend() {
    use std::sync::atomic::AtomicBool;
    let bytes = relocation_object();
    let unit = RelocatableUnit::parse(
        &bytes,
        "example",
        &RebuildLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(unit.relocations.len(), 1);
    let r = &unit.relocations[0];
    assert_eq!(
        (r.section, r.offset, r.raw_kind, r.symbol, r.addend),
        (1, 1, 2, 2, -7)
    );
}
#[test]
fn unit_rejects_bad_relocations_and_overlaps() {
    use std::sync::atomic::AtomicBool;
    let c = AtomicBool::new(false);
    let limits = RebuildLimits::default();
    let base = relocation_object();
    for (offset, value) in [
        (0x300, 6u64),
        (0x308, (99u64 << 32) | 2),
        (0x308, (2u64 << 32) | 99),
    ] {
        let mut b = base.clone();
        put64(&mut b, offset, value);
        assert!(RelocatableUnit::parse(&b, "example", &limits, &c).is_err());
    }
    let mut b = base.clone();
    put32(&mut b, 0x4c0 + 4, 9);
    assert!(RelocatableUnit::parse(&b, "example", &limits, &c).is_err());
    let mut b = base.clone();
    put64(&mut b, 0x4c0 + 32, 48);
    put64(&mut b, 0x318, 3);
    put64(&mut b, 0x320, (2u64 << 32) | 4);
    assert_eq!(
        RelocatableUnit::parse(&b, "example", &limits, &c)
            .unwrap_err()
            .code,
        RebuildErrorCode::Overlap
    );
    let mut b = base.clone();
    put64(&mut b, 0x308, 99u64 << 32);
    assert!(RelocatableUnit::parse(&b, "example", &limits, &c).is_err());
}
#[test]
fn unit_readonly_and_exact_limits() {
    use std::sync::atomic::AtomicBool;
    let b = readonly_object();
    let c = AtomicBool::new(false);
    let limits = RebuildLimits {
        max_object_bytes: b.len(),
        max_object_sections: 7,
        max_symbols: 3,
        max_relocations: 1,
        max_text_bytes: 8,
        max_rodata_bytes: 4,
        ..RebuildLimits::default()
    };
    let unit = RelocatableUnit::parse(&b, "example", &limits, &c).unwrap();
    assert_eq!(unit.sections.len(), 2);
    for smaller in [
        RebuildLimits {
            max_object_bytes: b.len() - 1,
            ..limits
        },
        RebuildLimits {
            max_object_sections: 6,
            ..limits
        },
        RebuildLimits {
            max_symbols: 2,
            ..limits
        },
        RebuildLimits {
            max_relocations: 0,
            ..limits
        },
        RebuildLimits {
            max_text_bytes: 7,
            ..limits
        },
        RebuildLimits {
            max_rodata_bytes: 3,
            ..limits
        },
    ] {
        assert_eq!(
            RelocatableUnit::parse(&b, "example", &smaller, &c)
                .unwrap_err()
                .code,
            RebuildErrorCode::TooLarge
        );
    }
}
#[test]
fn unit_rejects_writable_tls_nobits_and_extra_code() {
    use std::sync::atomic::AtomicBool;
    let b = readonly_object();
    let c = AtomicBool::new(false);
    for (offset, value) in [(0x500 + 8, 3u64), (0x500 + 8, 0x402), (0x500 + 8, 6)] {
        let mut b = b.clone();
        put64(&mut b, offset, value);
        assert!(RelocatableUnit::parse(&b, "example", &RebuildLimits::default(), &c).is_err());
    }
    let mut b = b.clone();
    put32(&mut b, 0x500 + 4, 8);
    assert!(RelocatableUnit::parse(&b, "example", &RebuildLimits::default(), &c).is_err());
    let mut b = b.clone();
    b[0x234] = 0x16;
    assert!(RelocatableUnit::parse(&b, "example", &RebuildLimits::default(), &c).is_err());
    let mut b = b.clone();
    b[0x234] = 0x1a;
    assert!(RelocatableUnit::parse(&b, "example", &RebuildLimits::default(), &c).is_err());
    let mut b = b.clone();
    put16(&mut b, 0x236, 0xfff2);
    assert!(RelocatableUnit::parse(&b, "example", &RebuildLimits::default(), &c).is_err());
}

#[test]
fn unit_rejects_empty_and_duplicate_function_definitions() {
    use std::sync::atomic::AtomicBool;
    let c = AtomicBool::new(false);
    let base = object_fixture();
    let mut empty = base.clone();
    put64(&mut empty, 0x3c0 + 32, 0);
    put64(&mut empty, 0x228, 0);
    assert!(RelocatableUnit::parse(&empty, "example", &RebuildLimits::default(), &c).is_err());
    let mut duplicate = base.clone();
    put64(&mut duplicate, 0x480 + 32, 72);
    duplicate[0x230..0x248].copy_from_slice(&base[0x218..0x230]);
    assert!(RelocatableUnit::parse(&duplicate, "example", &RebuildLimits::default(), &c).is_err());
    let mut alias = duplicate;
    alias[0x1c9..0x1cf].copy_from_slice(b"alias\0");
    put64(&mut alias, 0x440 + 32, 15);
    put32(&mut alias, 0x230, 9);
    assert!(RelocatableUnit::parse(&alias, "example", &RebuildLimits::default(), &c).is_err());
}

#[test]
fn relocation_pc32_checks_boundaries_and_explicit_addends() {
    use super::relocate::evaluate_pc32;
    assert_eq!(
        evaluate_pc32(0x2000, -4, 0x1001).unwrap(),
        [0xfb, 0x0f, 0, 0]
    );
    assert_eq!(
        evaluate_pc32(i32::MAX as u64, 0, 0).unwrap(),
        i32::MAX.to_le_bytes()
    );
    assert_eq!(
        evaluate_pc32(0, 0, 0x8000_0000).unwrap(),
        i32::MIN.to_le_bytes()
    );
    assert_eq!(
        evaluate_pc32(0x8000_0000, 0, 0).unwrap_err().code,
        RebuildErrorCode::RelocationOverflow
    );
    assert_eq!(
        evaluate_pc32(0, 0, 0x8000_0001).unwrap_err().code,
        RebuildErrorCode::RelocationOverflow
    );
    assert_eq!(evaluate_pc32(100, 9, 105).unwrap(), 4i32.to_le_bytes());
    assert_eq!(evaluate_pc32(100, -9, 105).unwrap(), (-14i32).to_le_bytes());
    assert_eq!(
        evaluate_pc32(u64::MAX, -4, u64::MAX - 8).unwrap(),
        4i32.to_le_bytes()
    );
    assert_eq!(
        evaluate_pc32(u64::MAX, i64::MAX, 0).unwrap_err().code,
        RebuildErrorCode::RelocationOverflow
    );
}

#[test]
fn binding_requires_unique_nonpreemptible_file_backed_definitions() {
    use super::relocate::{Definition, DefinitionKind, bind_external};
    let good = Definition {
        name: "helper".into(),
        address: 0x401100,
        size: 12,
        kind: DefinitionKind::Function,
        binding: 0,
        visibility: 0,
        file_backed: true,
        readable: true,
        executable: true,
        writable: false,
        dynamic_relocated: false,
    };
    assert_eq!(
        bind_external(
            std::slice::from_ref(&good),
            "helper",
            DefinitionKind::Function,
            false
        )
        .unwrap(),
        0x401100
    );
    let mut bad = good.clone();
    bad.binding = 2;
    assert!(bind_external(&[bad], "helper", DefinitionKind::Function, false).is_err());
    let mut bad = good.clone();
    bad.binding = 1;
    assert!(bind_external(&[bad], "helper", DefinitionKind::Function, false).is_err());
    let mut hidden = good.clone();
    hidden.binding = 1;
    hidden.visibility = 2;
    assert!(bind_external(&[hidden], "helper", DefinitionKind::Function, false).is_ok());
    let mut bad = good.clone();
    bad.file_backed = false;
    assert!(bind_external(&[bad], "helper", DefinitionKind::Function, false).is_err());
    assert_eq!(
        bind_external(
            &[good.clone(), good.clone()],
            "helper",
            DefinitionKind::Function,
            false
        )
        .unwrap_err()
        .code,
        RebuildErrorCode::AmbiguousSymbol
    );
    assert_eq!(
        bind_external(&[good], "imported", DefinitionKind::Function, false)
            .unwrap_err()
            .code,
        RebuildErrorCode::UndefinedSymbol
    );
}
#[test]
fn binding_data_requires_known_file_backed_span_and_permissions() {
    use super::relocate::{Definition, DefinitionKind, bind_external};
    let data = Definition {
        name: "counter".into(),
        address: 0x404010,
        size: 4,
        kind: DefinitionKind::Data,
        binding: 0,
        visibility: 0,
        file_backed: true,
        readable: true,
        executable: false,
        writable: true,
        dynamic_relocated: false,
    };
    assert!(
        bind_external(
            std::slice::from_ref(&data),
            "counter",
            DefinitionKind::Data,
            true
        )
        .is_ok()
    );
    for bad in [
        Definition {
            size: 0,
            ..data.clone()
        },
        Definition {
            file_backed: false,
            ..data.clone()
        },
        Definition {
            writable: false,
            ..data.clone()
        },
        Definition {
            dynamic_relocated: true,
            ..data.clone()
        },
    ] {
        assert!(bind_external(&[bad], "counter", DefinitionKind::Data, true).is_err());
    }
}

#[test]
fn relocation_sites_require_real_call_or_rip_displacement_fields() {
    use super::relocate::{RelocationSite, classify_site};
    assert_eq!(
        classify_site(&[0xe8, 0, 0, 0, 0, 0xc3], 1, 4).unwrap(),
        RelocationSite::Call
    );
    assert_eq!(
        classify_site(&[0x8b, 0x05, 0, 0, 0, 0, 0xc3], 2, 2).unwrap(),
        RelocationSite::Data {
            write: false,
            width: 4,
            next_ip_delta: 4
        }
    );
    assert_eq!(
        classify_site(&[0x89, 0x05, 0, 0, 0, 0, 0xc3], 2, 2).unwrap(),
        RelocationSite::Data {
            write: true,
            width: 4,
            next_ip_delta: 4
        }
    );
    assert!(classify_site(&[0xe9, 0, 0, 0, 0], 1, 4).is_err());
    assert!(classify_site(&[0xff, 0x15, 0, 0, 0, 0], 2, 2).is_err());
    assert!(classify_site(&[0xe8, 0, 0, 0, 0, 0xc3], 0, 4).is_err());
    assert!(classify_site(&[0x8b, 0x05, 0, 0, 0, 0, 0xc3], 2, 4).is_err());
}

#[test]
fn admission_dynamic_relocations_validate_raw_ranges_and_symbol_indices() {
    use super::dynamic::parse_table;
    let mut bytes = vec![0u8; 24];
    put64(&mut bytes, 0, 0x401005);
    put64(&mut bytes, 8, (2u64 << 32) | 7);
    let ranges = parse_table(&bytes, true, 3, &[0, 4, 64]).unwrap();
    assert_eq!(ranges, vec![0x401005..0x40100d]);
    assert!(parse_table(&bytes[..23], true, 3, &[0, 4, 64]).is_err());
    put64(&mut bytes, 8, (3u64 << 32) | 7);
    assert!(parse_table(&bytes, true, 3, &[0, 4, 64]).is_err());
    put64(&mut bytes, 8, (2u64 << 32) | 5);
    assert_eq!(
        parse_table(&bytes, true, 3, &[0, 4, 64]).unwrap(),
        vec![0x401005..0x401045]
    );
    put64(&mut bytes, 0, u64::MAX - 2);
    assert!(parse_table(&bytes, true, 3, &[0, 4, 64]).is_err());
}

#[test]
fn admission_dynamic_tags_must_match_validated_mapped_sections() {
    use super::{dynamic::inspect_dynamic, elf::ProgramHeader};
    let mut b = vec![0u8; 0x600];
    let h = vec![
        ProgramHeader {
            kind: 1,
            flags: 4,
            offset: 0,
            address: 0x400000,
            file_size: 0x600,
            memory_size: 0x600,
            alignment: 0x1000,
        },
        ProgramHeader {
            kind: 2,
            flags: 4,
            offset: 0x100,
            address: 0x400100,
            file_size: 128,
            memory_size: 128,
            alignment: 8,
        },
    ];
    for (i, (tag, value)) in [
        (6, 0x400200),
        (11, 24),
        (5, 0x400280),
        (10, 16),
        (7, 0x400300),
        (8, 24),
        (9, 24),
        (0, 0),
    ]
    .iter()
    .enumerate()
    {
        put64(&mut b, 0x100 + i * 16, *tag);
        put64(&mut b, 0x108 + i * 16, *value);
    }
    let s = 0x440;
    put32(&mut b, s + 4, 11);
    put64(&mut b, s + 16, 0x400200);
    put64(&mut b, s + 24, 0x200);
    put64(&mut b, s + 32, 48);
    put32(&mut b, s + 40, 2);
    put64(&mut b, s + 56, 24);
    let s = 0x480;
    put32(&mut b, s + 4, 3);
    put64(&mut b, s + 16, 0x400280);
    put64(&mut b, s + 24, 0x280);
    put64(&mut b, s + 32, 16);
    let s = 0x4c0;
    put32(&mut b, s + 4, 4);
    put64(&mut b, s + 16, 0x400300);
    put64(&mut b, s + 24, 0x300);
    put64(&mut b, s + 32, 24);
    put32(&mut b, s + 40, 1);
    put64(&mut b, s + 56, 24);
    put64(&mut b, 0x300, 0x400370);
    put64(&mut b, 0x308, (1u64 << 32) | 7);
    assert_eq!(
        inspect_dynamic(&b, &h, 0x400, 4, 4096).unwrap(),
        vec![0x400370..0x400378]
    );
    let mut bad = b.clone();
    put64(&mut bad, 0x148, 0x400301);
    assert!(inspect_dynamic(&bad, &h, 0x400, 4, 4096).is_err());
    let mut bad = b.clone();
    put64(&mut bad, 0x4c0 + 32, 23);
    assert!(inspect_dynamic(&bad, &h, 0x400, 4, 4096).is_err());
    let mut bad = b.clone();
    put64(&mut bad, 0x108, 0x400210);
    assert!(inspect_dynamic(&bad, &h, 0x400, 4, 4096).is_err());
}

#[test]
fn binding_definitions_come_from_the_validated_image_mapping() {
    let (image, site) = loadable_fixture();
    let layout = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let d = layout
        .definitions
        .iter()
        .find(|d| d.name == "example")
        .unwrap();
    assert_eq!((d.address, d.size), (0x401000, 8));
    assert!(d.file_backed && d.executable && d.readable && !d.dynamic_relocated);
}

#[test]
fn relocation_resolves_internal_rodata_and_rejects_missing_imports() {
    use std::{collections::BTreeMap, sync::atomic::AtomicBool};
    let (image, site) = loadable_fixture();
    let original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let mut b = readonly_object();
    // mov eax,[rip+disp32]; ret, with relocation to section-symbol 2 in rodata.
    b[0x100..0x108].copy_from_slice(&[0x8b, 0x05, 0, 0, 0, 0, 0xc3, 0xc3]);
    put64(&mut b, 0x300, 2);
    put64(&mut b, 0x308, (2u64 << 32) | 2);
    b[0x310..0x318].copy_from_slice(&(-4i64).to_le_bytes());
    b[0x234] = 3;
    put16(&mut b, 0x236, 6);
    let unit = RelocatableUnit::parse(
        &b,
        "example",
        &RebuildLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let placement = super::layout::AppendLayout {
        section_addresses: BTreeMap::from([(1, 0x500000), (6, 0x4ff000)]),
        body_address: 0x500000,
        ..Default::default()
    };
    let resolved =
        super::relocate::resolve_unit(&unit, &original, &placement, &AtomicBool::new(false))
            .unwrap();
    assert_eq!(&resolved.sections[&1][2..6], &(-4102i32).to_le_bytes());
    assert_eq!(resolved.relocations[0].target_address, 0x4ff000);
    let mut missing = b.clone();
    put16(&mut missing, 0x236, 0);
    missing[0x234] = 0x10;
    let unit = RelocatableUnit::parse(
        &missing,
        "example",
        &RebuildLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        super::relocate::resolve_unit(&unit, &original, &placement, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        RebuildErrorCode::UndefinedSymbol
    );
}

#[test]
fn relocation_resolves_local_calls_self_recursion_and_data_permissions() {
    use super::relocate::{Definition, DefinitionKind, resolve_unit};
    use std::{collections::BTreeMap, sync::atomic::AtomicBool};
    let (image, site) = loadable_fixture();
    let mut original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let placement = super::layout::AppendLayout {
        section_addresses: BTreeMap::from([(1, 0x500000)]),
        body_address: 0x500000,
        ..Default::default()
    };
    let c = AtomicBool::new(false);
    let mut b = relocation_object();
    b[0x100..0x108].copy_from_slice(&[0xe8, 0, 0, 0, 0, 0xc3, 0xc3, 0xc3]);
    put64(&mut b, 0x308, (2u64 << 32) | 4);
    b[0x310..0x318].copy_from_slice(&(-4i64).to_le_bytes());
    original.definitions.push(Definition {
        name: "ext".into(),
        address: 0x4ff000,
        size: 8,
        kind: DefinitionKind::Function,
        binding: 0,
        visibility: 0,
        file_backed: true,
        readable: true,
        executable: true,
        writable: false,
        dynamic_relocated: false,
    });
    let unit = RelocatableUnit::parse(&b, "example", &RebuildLimits::default(), &c).unwrap();
    let resolved = resolve_unit(&unit, &original, &placement, &c).unwrap();
    assert_eq!(&resolved.sections[&1][1..5], &(-4101i32).to_le_bytes());
    let mut self_call = b.clone();
    put64(&mut self_call, 0x308, (1u64 << 32) | 4);
    let unit =
        RelocatableUnit::parse(&self_call, "example", &RebuildLimits::default(), &c).unwrap();
    assert_eq!(
        &resolve_unit(&unit, &original, &placement, &c)
            .unwrap()
            .sections[&1][1..5],
        &(-5i32).to_le_bytes()
    );
    let mut interior = b.clone();
    interior[0x310..0x318].copy_from_slice(&(-3i64).to_le_bytes());
    let unit = RelocatableUnit::parse(&interior, "example", &RebuildLimits::default(), &c).unwrap();
    assert!(resolve_unit(&unit, &original, &placement, &c).is_err());
    let mut data = b;
    data[0x100..0x108].copy_from_slice(&[0x89, 0x05, 0, 0, 0, 0, 0xc3, 0xc3]);
    put64(&mut data, 0x300, 2);
    put64(&mut data, 0x308, (2u64 << 32) | 2);
    original.definitions.pop();
    original.definitions.push(Definition {
        name: "ext".into(),
        address: 0x4ff000,
        size: 4,
        kind: DefinitionKind::Data,
        binding: 0,
        visibility: 0,
        file_backed: true,
        readable: true,
        executable: false,
        writable: true,
        dynamic_relocated: false,
    });
    let unit = RelocatableUnit::parse(&data, "example", &RebuildLimits::default(), &c).unwrap();
    assert!(resolve_unit(&unit, &original, &placement, &c).is_ok());
    original.definitions.last_mut().unwrap().writable = false;
    assert!(resolve_unit(&unit, &original, &placement, &c).is_err());
    original.definitions.last_mut().unwrap().writable = true;
    original.definitions.last_mut().unwrap().size = 3;
    assert!(resolve_unit(&unit, &original, &placement, &c).is_err());
    assert_eq!(
        resolve_unit(&unit, &original, &placement, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        RebuildErrorCode::Cancelled
    );
}

#[test]
fn relocation_data_bounds_account_for_immediates_after_displacement() {
    use super::relocate::{Definition, DefinitionKind, resolve_unit};
    use std::{collections::BTreeMap, sync::atomic::AtomicBool};
    let (image, site) = loadable_fixture();
    let mut original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    original.definitions.push(Definition {
        name: "ext".into(),
        address: 0x4ff000,
        size: 1,
        kind: DefinitionKind::Data,
        binding: 0,
        visibility: 0,
        file_backed: true,
        readable: true,
        executable: false,
        writable: true,
        dynamic_relocated: false,
    });
    let placement = super::layout::AppendLayout {
        section_addresses: BTreeMap::from([(1, 0x500000)]),
        body_address: 0x500000,
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    // Both instructions have an imm8 after their disp32: next RIP is P+5.
    for opcode in [0x80, 0xc6] {
        let mut object = relocation_object();
        let modrm = if opcode == 0x80 { 0x3d } else { 0x05 };
        object[0x100..0x108].copy_from_slice(&[opcode, modrm, 0, 0, 0, 0, 1, 0xc3]);
        put64(&mut object, 0x300, 2);
        put64(&mut object, 0x308, (2u64 << 32) | 2);
        object[0x310..0x318].copy_from_slice(&(-5i64).to_le_bytes());
        let unit =
            RelocatableUnit::parse(&object, "example", &RebuildLimits::default(), &cancel).unwrap();
        assert!(
            resolve_unit(&unit, &original, &placement, &cancel).is_ok(),
            "valid opcode {opcode:x}"
        );
        // A forged -4 addend accesses one byte past the one-byte symbol.
        object[0x310..0x318].copy_from_slice(&(-4i64).to_le_bytes());
        let unit =
            RelocatableUnit::parse(&object, "example", &RebuildLimits::default(), &cancel).unwrap();
        assert!(
            resolve_unit(&unit, &original, &placement, &cancel).is_err(),
            "out of bounds opcode {opcode:x}"
        );
    }
}

#[test]
fn relocation_rejects_unrelocated_rip_memory_and_outgoing_jumps() {
    use std::{collections::BTreeMap, sync::atomic::AtomicBool};
    let (image, site) = loadable_fixture();
    let original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let placement = super::layout::AppendLayout {
        section_addresses: BTreeMap::from([(1, 0x500000)]),
        body_address: 0x500000,
        ..Default::default()
    };
    for code in [
        [0x8b, 0x05, 0, 0, 0, 0, 0xc3, 0xc3],
        [0xe9, 0x20, 0, 0, 0, 0xc3, 0xc3, 0xc3],
        [0xff, 0xd0, 0xc3, 0xc3, 0xc3, 0xc3, 0xc3, 0xc3],
    ] {
        let mut b = object_fixture();
        b[0x100..0x108].copy_from_slice(&code);
        let unit = RelocatableUnit::parse(
            &b,
            "example",
            &RebuildLimits::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            super::relocate::resolve_unit(&unit, &original, &placement, &AtomicBool::new(false))
                .is_err()
        );
    }
}

#[test]
fn metadata_rewrite_note_roundtrips_and_rejects_malformed_records() {
    use super::metadata::{RewriteLocation, decode_note, encode_note};
    let loc = RewriteLocation {
        entry_address: 0x401000,
        original_capacity: 8,
        body_address: 0x403000,
        body_size: 72,
    };
    let note = encode_note(&loc);
    assert_eq!(note.len(), 52);
    assert_eq!(decode_note(&note).unwrap(), loc);
    for offset in [0, 4, 8, 12, 16] {
        let mut bad = note.clone();
        bad[offset] ^= 0x80;
        assert!(decode_note(&bad).is_err());
    }
    assert!(decode_note(&note[..51]).is_err());
    let mut trailing = note.clone();
    trailing.extend_from_slice(&[0, 0, 0, 0]);
    assert!(decode_note(&trailing).is_err());
}

#[test]
fn layout_appends_phdr_rodata_text_and_bounded_metadata() {
    use std::sync::atomic::AtomicBool;
    let (image, site) = loadable_fixture();
    let original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let unit = RelocatableUnit::parse(
        &readonly_object(),
        "example",
        &RebuildLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let plan = super::layout::plan_layout(&original, &unit, &RebuildLimits::default()).unwrap();
    assert_eq!(plan.phdr_count, 4);
    assert_eq!(plan.phdr_file.start, 0x1000);
    assert_eq!(plan.phdr_address, 0x402000);
    assert_eq!(plan.section_addresses[&6], 0x4020e0);
    assert_eq!(plan.body_address, 0x403000);
    assert_eq!(plan.section_files[&1], 0x2000..0x2008);
    assert_eq!(plan.detour, [0xe9, 0xfb, 0x1f, 0, 0, 0x90, 0x90, 0x90]);
    assert!(plan.note_file.start >= plan.section_files[&1].end);
    assert!(plan.output_len <= RebuildLimits::default().max_image_bytes);
    assert_eq!(
        plan,
        super::layout::plan_layout(&original, &unit, &RebuildLimits::default()).unwrap()
    );
    let exact = RebuildLimits {
        max_image_bytes: plan.output_len,
        ..RebuildLimits::default()
    };
    assert!(super::layout::plan_layout(&original, &unit, &exact).is_ok());
    let over = RebuildLimits {
        max_image_bytes: plan.output_len - 1,
        ..exact
    };
    assert_eq!(
        super::layout::plan_layout(&original, &unit, &over)
            .unwrap_err()
            .code,
        RebuildErrorCode::TooLarge
    );
}

#[test]
fn layout_handles_no_rodata_and_rejects_alignment_headers_and_reachability() {
    use std::sync::atomic::AtomicBool;
    let (image, site) = loadable_fixture();
    let mut original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let mut unit = RelocatableUnit::parse(
        &object_fixture(),
        "example",
        &RebuildLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let plan = super::layout::plan_layout(&original, &unit, &RebuildLimits::default()).unwrap();
    assert_eq!(plan.read_file, 0x1000..0x10e0);
    assert_eq!(plan.section_files.len(), 1);
    unit.sections[0].alignment = 1 << 30;
    assert_eq!(
        super::layout::plan_layout(&original, &unit, &RebuildLimits::default())
            .unwrap_err()
            .code,
        RebuildErrorCode::TooLarge
    );
    unit.sections[0].alignment = 1;
    original.program_headers[0].address = 0x9000_0000;
    assert_eq!(
        super::layout::plan_layout(&original, &unit, &RebuildLimits::default())
            .unwrap_err()
            .code,
        RebuildErrorCode::RelocationOverflow
    );
    original.program_headers[0].address = 0x401000;
    let template = original.program_headers[0].clone();
    original
        .program_headers
        .resize_with(256, || template.clone());
    assert_eq!(
        super::layout::plan_layout(&original, &unit, &RebuildLimits::default())
            .unwrap()
            .phdr_count,
        259
    );
    original.program_headers.push(template);
    assert_eq!(
        super::layout::plan_layout(&original, &unit, &RebuildLimits::default())
            .unwrap_err()
            .code,
        RebuildErrorCode::TooLarge
    );
}

fn writer_fixture() -> (
    BinaryImage,
    ReplacementSite,
    super::layout::AppendLayout,
    super::relocate::ResolvedUnit,
) {
    use std::sync::atomic::AtomicBool;
    let (image, site) = loadable_fixture();
    let original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let mut bytes = object_fixture();
    bytes[0x100..0x108].copy_from_slice(&[0xb8, 2, 0, 0, 0, 0xc3, 0xc3, 0xc3]);
    let unit = RelocatableUnit::parse(
        &bytes,
        "example",
        &RebuildLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let plan = super::layout::plan_layout(&original, &unit, &RebuildLimits::default()).unwrap();
    let resolved =
        super::relocate::resolve_unit(&unit, &original, &plan, &AtomicBool::new(false)).unwrap();
    (image, site, plan, resolved)
}
#[test]
fn writer_preserves_original_prefix_outside_whitelist() {
    let (image, site, plan, resolved) = writer_fixture();
    let original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let candidate = super::elf::write_candidate(&image, &original, &plan, &resolved).unwrap();
    assert_eq!(candidate.len(), plan.output_len);
    for (offset, (before, after)) in image.bytes().iter().zip(&candidate).enumerate() {
        if before != after {
            assert!(
                plan.changed_original_ranges
                    .iter()
                    .any(|r| r.contains(&offset)),
                "unexpected byte at {offset:#x}"
            );
        }
    }
    assert_eq!(
        &candidate[site.offset..site.offset + site.capacity],
        plan.detour
    );
    let parsed = BinaryImage::parse(candidate).unwrap();
    assert_eq!(
        parsed.address_to_offset(plan.body_address),
        Some(plan.text_file.start)
    );
}

#[test]
fn writer_validator_rejects_prefix_header_and_segment_corruption() {
    let (image, site, plan, resolved) = writer_fixture();
    let original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let candidate = super::elf::write_candidate(&image, &original, &plan, &resolved).unwrap();
    assert!(
        super::validate::validate_candidate(&image, &original, &plan, &resolved, &candidate)
            .is_ok()
    );
    for offset in [0x300, 0x20, plan.phdr_file.start + 4, plan.text_file.start] {
        let mut bad = candidate.clone();
        bad[offset] ^= 0x80;
        assert!(
            super::validate::validate_candidate(&image, &original, &plan, &resolved, &bad).is_err(),
            "offset {offset:#x}"
        );
    }
    assert!(
        super::validate::validate_candidate(
            &image,
            &original,
            &plan,
            &resolved,
            &candidate[..candidate.len() - 1]
        )
        .is_err()
    );
}

#[test]
fn writer_planner_binds_whole_candidate_and_rejects_stale_base() {
    use std::sync::atomic::AtomicBool;
    let (image, site) = loadable_fixture();
    let mut object = object_fixture();
    object[0x100..0x108].copy_from_slice(&[0xb8, 2, 0, 0, 0, 0xc3, 0xc3, 0xc3]);
    let cancel = AtomicBool::new(false);
    let unit =
        RelocatableUnit::parse(&object, "example", &RebuildLimits::default(), &cancel).unwrap();
    let options = RebuildOptions {
        mode: PlacementMode::Relocated,
        limits: RebuildLimits::default(),
    };
    let plan = plan_replacement(&image, &site, &unit, options, &cancel).unwrap();
    assert_eq!(plan.summary().mode, PlacementMode::Relocated);
    assert!(plan.candidate().bytes().len() > image.bytes().len());
    assert_eq!(
        plan.apply_to(&image).unwrap().bytes(),
        plan.candidate().bytes()
    );
    let changed = image.patched(0x300, &[1]).unwrap();
    assert_eq!(
        plan.apply_to(&changed).unwrap_err().code,
        RebuildErrorCode::StaleImage
    );
    let mut bad_site = site.clone();
    bad_site.name = "other".into();
    assert!(plan_replacement(&image, &bad_site, &unit, options, &cancel).is_err());
    assert_eq!(
        plan_replacement(&image, &site, &unit, options, &AtomicBool::new(true))
            .err()
            .unwrap()
            .code,
        RebuildErrorCode::Cancelled
    );
}

#[test]
fn writer_maps_phdr_table_orders_loads_and_has_no_wx() {
    let (image, site, plan, resolved) = writer_fixture();
    let original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let candidate = super::elf::write_candidate(&image, &original, &plan, &resolved).unwrap();
    let get32 = |o| u32::from_le_bytes(candidate[o..o + 4].try_into().unwrap());
    let get64 = |o| u64::from_le_bytes(candidate[o..o + 8].try_into().unwrap());
    let p = plan.phdr_file.start;
    assert_eq!(get32(p), 6);
    assert_eq!(get64(p + 8), p as u64);
    assert_eq!(get64(p + 16), plan.phdr_address);
    let r = p + (plan.phdr_count - 2) * 56;
    let x = r + 56;
    assert_eq!((get32(r), get32(r + 4)), (1, 4));
    assert_eq!((get32(x), get32(x + 4)), (1, 5));
    assert!(get64(r + 16) < get64(x + 16));
    assert!(
        super::validate::validate_candidate(&image, &original, &plan, &resolved, &candidate)
            .is_ok()
    );
    let mut corrupt = candidate.clone();
    put32(&mut corrupt, x + 4, 7);
    assert!(
        super::validate::validate_candidate(&image, &original, &plan, &resolved, &corrupt).is_err()
    );
}
#[test]
fn writer_preserves_unrelated_relro_program_header() {
    use std::sync::atomic::AtomicBool;
    let (image, _) = loadable_fixture();
    let mut bytes = image.bytes().to_vec();
    put16(&mut bytes, 56, 2);
    let p = 120;
    put32(&mut bytes, p, 0x6474e552);
    put32(&mut bytes, p + 4, 4);
    put64(&mut bytes, p + 8, 0x200);
    put64(&mut bytes, p + 16, 0x402000);
    put64(&mut bytes, p + 24, 0x402000);
    put64(&mut bytes, p + 32, 16);
    put64(&mut bytes, p + 40, 16);
    put64(&mut bytes, p + 48, 1);
    let image = BinaryImage::parse(bytes).unwrap();
    let site = ReplacementSite::discover(&image, 0x401000).unwrap();
    let original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let mut obj = object_fixture();
    obj[0x100..0x108].copy_from_slice(&[0xb8, 2, 0, 0, 0, 0xc3, 0xc3, 0xc3]);
    let unit = RelocatableUnit::parse(
        &obj,
        "example",
        &RebuildLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let plan = super::layout::plan_layout(&original, &unit, &RebuildLimits::default()).unwrap();
    let resolved =
        super::relocate::resolve_unit(&unit, &original, &plan, &AtomicBool::new(false)).unwrap();
    let candidate = super::elf::write_candidate(&image, &original, &plan, &resolved).unwrap();
    assert_eq!(
        &candidate[plan.phdr_file.start + 2 * 56..plan.phdr_file.start + 3 * 56],
        &image.bytes()[120..176]
    );
    assert!(
        super::validate::validate_candidate(&image, &original, &plan, &resolved, &candidate)
            .is_ok()
    );
}
#[test]
fn writer_rejects_bad_symtab_links_and_invalid_local_order() {
    let (image, site, plan, resolved) = writer_fixture();
    let mut bad = image.clone();
    let mut bytes = image.bytes().to_vec();
    put32(&mut bytes, 0x480 + 40, 99);
    bad.bytes = bytes.into();
    assert!(super::elf::inspect_image(&bad, &site, &RebuildLimits::default()).is_err());
    let mut bytes = image.bytes().to_vec();
    bytes[0x21c] = 0x02;
    let mut bad = image.clone();
    bad.bytes = bytes.into();
    let original = super::elf::inspect_image(&bad, &site, &RebuildLimits::default()).unwrap();
    assert!(super::elf::write_candidate(&bad, &original, &plan, &resolved).is_err());
}
#[test]
fn admission_rejects_entry_overlap_with_other_section() {
    let (image, site) = loadable_fixture();
    let mut bytes = image.bytes().to_vec();
    put64(&mut bytes, 0x440 + 24, 0x100);
    let mut bad = image.clone();
    bad.bytes = bytes.into();
    assert!(super::elf::inspect_image(&bad, &site, &RebuildLimits::default()).is_err());
}

#[test]
fn metadata_reopened_image_cross_checks_note_entry_and_body() {
    let (image, site, layout, resolved) = writer_fixture();
    let original = super::elf::inspect_image(&image, &site, &RebuildLimits::default()).unwrap();
    let candidate = super::elf::write_candidate(&image, &original, &layout, &resolved).unwrap();
    let reopened = BinaryImage::parse(candidate.clone()).unwrap();
    assert_eq!(
        reopened.rewrite_locations(),
        &[RewriteLocation {
            entry_address: site.address,
            original_capacity: site.capacity as u64,
            body_address: layout.body_address,
            body_size: layout.text_file.len() as u64
        }]
    );
    assert_eq!(
        reopened.address_to_offset(layout.body_address),
        Some(layout.text_file.start)
    );
    for offset in [
        layout.note_file.start,
        layout.note_file.start + 16,
        layout.note_file.start + 36,
        site.offset,
    ] {
        let mut bad = candidate.clone();
        bad[offset] ^= 0x40;
        let reopened = BinaryImage::parse(bad).unwrap();
        assert!(reopened.rewrite_locations().is_empty());
        assert!(reopened.source_locations.is_empty());
        assert!(
            reopened
                .warnings
                .iter()
                .any(|w| w.contains("rewrite metadata"))
        );
        assert!(super::elf::inspect_image(&reopened, &site, &RebuildLimits::default()).is_err());
    }
}

#[test]
fn relocation_pc32_is_invariant_under_shared_load_bias() {
    let expected = super::relocate::evaluate_pc32(0x2000, -4, 0x1001).unwrap();
    for bias in [0u64, 0x10000000, 0x7f0000000000] {
        assert_eq!(
            super::relocate::evaluate_pc32(0x2000 + bias, -4, 0x1001 + bias).unwrap(),
            expected
        );
    }
}
#[test]
fn admission_pie_requires_interpreter_and_pie_identity() {
    let (image, site) = loadable_fixture();
    let mut bytes = image.bytes().to_vec();
    put16(&mut bytes, 16, 3);
    let mut candidate = image.clone();
    candidate.bytes = bytes.into();
    assert!(super::elf::inspect_image(&candidate, &site, &RebuildLimits::default()).is_err());
}

#[test]
fn admission_dynamic_relocation_widths_match_x86_64_abi() {
    let mut b = vec![0u8; 24];
    put64(&mut b, 0, 0x4018);
    for (kind, width) in [
        (1u64, 8u64),
        (2, 4),
        (6, 8),
        (7, 8),
        (8, 8),
        (16, 8),
        (17, 8),
        (18, 8),
        (36, 16),
        (37, 8),
        (38, 8),
    ] {
        put64(&mut b, 8, kind);
        assert_eq!(
            super::dynamic::parse_table(&b, true, 1, &[0]).unwrap(),
            vec![0x4018..0x4018 + width]
        );
    }
}
