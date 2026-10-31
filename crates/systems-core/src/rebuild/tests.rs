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
