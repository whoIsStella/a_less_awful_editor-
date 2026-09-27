use super::*;

// Independently laid-out ELF64 executable: .text, .bss and .shstrtab.
// Tests do not inspect or modify source files or invoke a compiler.
fn elf(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; 0x300];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    put16(&mut bytes, 16, 2);
    put16(&mut bytes, 18, machine);
    put32(&mut bytes, 20, 1);
    put64(&mut bytes, 24, 0x401000);
    put64(&mut bytes, 40, 0x200);
    put16(&mut bytes, 52, 64);
    put16(&mut bytes, 58, 64);
    put16(&mut bytes, 60, 4);
    put16(&mut bytes, 62, 3);
    bytes[0x100..0x10c].copy_from_slice(&[
        0x55, 0x48, 0x89, 0xe5, 0xb8, 0x2a, 0, 0, 0, 0x5d, 0xc3, 0x90,
    ]);
    let names = b"\0.text\0.bss\0.shstrtab\0";
    bytes[0x180..0x180 + names.len()].copy_from_slice(names);
    section(&mut bytes, 0x240, (1, 1), 6, 0x401000, 0x100, 12);
    section(&mut bytes, 0x280, (7, 8), 3, 0x402000, 0x170, 64);
    section(&mut bytes, 0x2c0, (12, 3), 0, 0, 0x180, names.len() as u64);
    bytes
}

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn section(
    bytes: &mut [u8],
    offset: usize,
    name_and_kind: (u32, u32),
    flags: u64,
    address: u64,
    file_offset: u64,
    size: u64,
) {
    let (name, kind) = name_and_kind;
    put32(bytes, offset, name);
    put32(bytes, offset + 4, kind);
    put64(bytes, offset + 8, flags);
    put64(bytes, offset + 16, address);
    put64(bytes, offset + 24, file_offset);
    put64(bytes, offset + 32, size);
    put64(bytes, offset + 48, 1);
}

#[test]
fn parses_elf_and_maps_only_file_backed_allocated_sections() {
    let image = BinaryImage::parse(elf(62)).unwrap();
    assert_eq!(image.format, BinaryFormat::Elf);
    assert_eq!(image.architecture, Architecture::X86_64);
    assert_eq!(image.entry, Some(0x401000));
    assert_eq!(image.offset_to_address(0x100), Some(0x401000));
    assert_eq!(image.offset_to_address(0x10b), Some(0x40100b));
    assert_eq!(image.offset_to_address(0x10c), None);
    assert_eq!(image.address_to_offset(0x401001), Some(0x101));
    assert_eq!(image.address_to_offset(0x402001), None);
    assert_eq!(image.offset_to_address(0x180), None);
    assert_eq!(
        image
            .sections
            .iter()
            .find(|s| s.name == ".bss")
            .unwrap()
            .file_size,
        0
    );
}

#[test]
fn decodes_known_x64_instructions_and_obeys_both_limits() {
    let image = BinaryImage::parse(elf(62)).unwrap();
    let instructions = image.disassemble(0x100, 1000, 50).unwrap();
    let text: Vec<_> = instructions.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(
        text,
        [
            "push rbp",
            "mov rbp,rsp",
            "mov eax,0x2a",
            "pop rbp",
            "ret",
            "nop"
        ]
    );
    assert_eq!(instructions.last().unwrap().offset, 0x10b);
    assert_eq!(image.disassemble(0x100, 1000, 2).unwrap().len(), 2);
    let truncated = image.disassemble(0x101, 2, 20).unwrap();
    assert!(truncated.iter().all(|i| !i.valid));
    assert_eq!(truncated.iter().map(|i| i.bytes.len()).sum::<usize>(), 2);
    assert!(image.disassemble(0, 10, 10).is_err());
}

#[test]
fn raw_requires_explicit_architecture_and_branches_keep_address() {
    let image = BinaryImage::parse(vec![0xeb, 2, 0x90, 0x90, 0xc3]).unwrap();
    assert!(image.disassemble(0, 10, 10).is_err());
    let instructions = image
        .disassemble_as(&Architecture::X86, 0, 100, 100)
        .unwrap();
    assert_eq!(instructions[0].branch_target, Some(4));
    assert_eq!(instructions[0].bytes, [0xeb, 2]);
    assert_eq!(instructions[3].text, "ret");
    assert!(
        image
            .disassemble_as(&Architecture::X86, usize::MAX, 1, 1)
            .is_err()
    );
}

#[test]
fn non_x86_metadata_is_available_but_not_fake_disassembly() {
    let image = BinaryImage::parse(elf(183)).unwrap();
    assert_eq!(image.architecture, Architecture::Other("Aarch64".into()));
    assert!(
        image
            .disassemble(0x100, 12, 6)
            .unwrap_err()
            .contains("Aarch64")
    );
    assert!(
        image
            .disassemble_as(&Architecture::X86_64, 0x100, 12, 6)
            .is_err()
    );
}

#[test]
fn malformed_ranges_and_truncated_headers_are_rejected() {
    for bytes in [
        b"\x7fELF".to_vec(),
        b"MZ".to_vec(),
        vec![0xcf, 0xfa, 0xed, 0xfe],
    ] {
        assert!(BinaryImage::parse(bytes).is_err());
    }
    let mut bytes = elf(62);
    put64(&mut bytes, 0x240 + 24, u64::MAX);
    assert!(BinaryImage::parse(bytes).is_err());
    let mut bytes = elf(62);
    put64(&mut bytes, 0x240 + 16, u64::MAX - 4);
    assert!(BinaryImage::parse(bytes).is_err());
}

#[test]
fn overlapping_address_and_file_ranges_are_not_guessed() {
    let mut bytes = elf(62);
    section(&mut bytes, 0x280, (7, 1), 3, 0x401000, 0x110, 8);
    let image = BinaryImage::parse(bytes).unwrap();
    assert_eq!(image.address_to_offset(0x401000), None);
    assert_eq!(image.offset_to_address(0x100), None);
    let mut bytes = elf(62);
    section(&mut bytes, 0x280, (7, 1), 3, 0x402000, 0x104, 8);
    let image = BinaryImage::parse(bytes).unwrap();
    assert_eq!(image.offset_to_address(0x104), None);
    let instructions = image.disassemble(0x100, 12, 20).unwrap();
    assert_eq!(instructions.iter().map(|i| i.bytes.len()).sum::<usize>(), 4);
}

#[test]
fn entropy_distinguishes_repetition_from_uniform_distribution() {
    let mut bytes = vec![0; 256];
    bytes.extend(0..=255);
    let image = BinaryImage::parse(bytes).unwrap();
    let bins = image.entropy(0..512, 256).unwrap();
    assert_eq!(bins[0].entropy, 0.0);
    assert_eq!(bins[1].entropy, 8.0);
    assert!(image.entropy(0..513, 256).is_err());
    assert!(image.entropy(0..512, 0).is_err());
    assert!(image.entropy(Range { start: 10, end: 5 }, 1).is_err());
}

#[test]
fn fixed_length_patch_reparses_without_mutating_old_snapshot() {
    let image = BinaryImage::parse(elf(62)).unwrap();
    let changed = image.patched(0x105, &[7]).unwrap();
    assert_eq!(image.bytes()[0x105], 42);
    assert_eq!(changed.bytes()[0x105], 7);
    assert_eq!(
        changed.disassemble(0x104, 5, 1).unwrap()[0].text,
        "mov eax,7"
    );
    assert!(image.patched(usize::MAX, &[1]).is_err());
    assert!(image.patched(image.bytes().len(), &[1]).is_err());
    assert!(image.patched(0x240 + 24, &u64::MAX.to_le_bytes()).is_err());
}

#[test]
fn printable_strings_are_bounded_and_use_file_offsets() {
    let image = BinaryImage::parse(b"\0hello\0xy\0world!\0".to_vec()).unwrap();
    assert_eq!(
        image.strings(4, 1),
        vec![PrintableString {
            offset: 1,
            text: "hello".into()
        }]
    );
    assert_eq!(image.strings(4, 10).len(), 2);
    assert!(image.strings(4, 0).is_empty());
}

#[test]
fn source_lookup_does_not_extend_past_sequence_or_ambiguous_ranges() {
    let mut image = BinaryImage::parse(elf(62)).unwrap();
    image.source_locations = vec![SourceLocation {
        path: "/tmp/main.c".into(),
        line: 3,
        column: 1,
        address: 0x401000,
        end_address: 0x401004,
        file_offset: Some(0x100),
    }];
    image.source_end_prefix = source_end_prefix(&image.source_locations);
    assert_eq!(image.source_at_offset(0x102).unwrap().line, 3);
    assert!(image.source_at_offset(0x104).is_none());
    image.source_locations.push(SourceLocation {
        path: "/tmp/other.c".into(),
        line: 9,
        column: 0,
        address: 0x401001,
        end_address: 0x401003,
        file_offset: Some(0x101),
    });
    image.source_end_prefix = source_end_prefix(&image.source_locations);
    assert!(image.source_at_offset(0x102).is_none());
}

fn elf_with_dwarf() -> Vec<u8> {
    let mut bytes = elf(62);
    bytes.resize(0xa00, 0);
    put64(&mut bytes, 40, 0x800);
    put16(&mut bytes, 60, 7);
    let names = b"\0.text\0.bss\0.shstrtab\0.debug_abbrev\0.debug_info\0.debug_line\0";
    bytes[0x180..0x180 + names.len()].copy_from_slice(names);
    section(&mut bytes, 0x840, (1, 1), 6, 0x401000, 0x100, 12);
    section(&mut bytes, 0x880, (7, 8), 3, 0x402000, 0x170, 64);
    section(&mut bytes, 0x8c0, (12, 3), 0, 0, 0x180, names.len() as u64);
    // DW_TAG_compile_unit with DW_AT_stmt_list and DW_AT_comp_dir.
    let abbrev = [1, 0x11, 0, 0x10, 0x17, 0x1b, 8, 0, 0, 0];
    bytes[0x300..0x300 + abbrev.len()].copy_from_slice(&abbrev);
    section(&mut bytes, 0x900, (22, 1), 0, 0, 0x300, abbrev.len() as u64);
    let mut info = vec![0; 4];
    info.extend(4u16.to_le_bytes());
    info.extend(0u32.to_le_bytes());
    info.extend([8, 1]);
    info.extend(0u32.to_le_bytes());
    info.extend(b"/fixtures\0");
    let length = info.len() as u32 - 4;
    put32(&mut info, 0, length);
    bytes[0x340..0x340 + info.len()].copy_from_slice(&info);
    section(&mut bytes, 0x940, (36, 1), 0, 0, 0x340, info.len() as u64);
    let mut header = vec![1, 1, 1, (-5i8) as u8, 14, 13];
    header.extend([0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
    header.extend(b"src\0\0demo.c\0");
    header.extend([1, 0, 0, 0]);
    let mut line = vec![0; 4];
    line.extend(4u16.to_le_bytes());
    line.extend((header.len() as u32).to_le_bytes());
    line.extend(header);
    line.extend([0, 9, 2]); // Extended set_address.
    line.extend(0x401000u64.to_le_bytes());
    line.extend([1, 2, 4, 3, 1, 1, 2, 8, 0, 1, 1]);
    let length = line.len() as u32 - 4;
    put32(&mut line, 0, length);
    bytes[0x400..0x400 + line.len()].copy_from_slice(&line);
    section(&mut bytes, 0x980, (48, 1), 0, 0, 0x400, line.len() as u64);
    bytes
}

#[test]
fn dwarf_fixture_connects_real_line_program_addresses_to_exact_bytes() {
    let image = BinaryImage::parse(elf_with_dwarf()).unwrap();
    assert!(image.warnings.is_empty(), "{:?}", image.warnings);
    assert_eq!(image.source_locations.len(), 2);
    let first = image.source_at_offset(0x100).unwrap();
    assert_eq!(first.path, "/fixtures/src/demo.c");
    assert_eq!(first.line, 1);
    assert_eq!(first.file_offset, Some(0x100));
    assert_eq!(first.end_address, 0x401004);
    assert_eq!(image.source_at_offset(0x104).unwrap().line, 2);
    assert_eq!(image.source_at_offset(0x10b).unwrap().line, 2);
    assert!(image.source_at_offset(0x10c).is_none());
}

#[test]
fn malformed_and_compressed_dwarf_preserve_image_with_explicit_warning() {
    let mut bytes = elf_with_dwarf();
    put32(&mut bytes, 0x400, u32::MAX);
    let image = BinaryImage::parse(bytes).unwrap();
    assert!(image.source_locations.is_empty());
    assert!(
        image
            .warnings
            .iter()
            .any(|warning| warning.contains("Source mapping unavailable"))
    );
    assert_eq!(image.disassemble(0x100, 1, 1).unwrap()[0].text, "push rbp");
    let mut bytes = elf_with_dwarf();
    put64(
        &mut bytes,
        0x980 + 8,
        u64::from(object::elf::SHF_COMPRESSED),
    );
    let image = BinaryImage::parse(bytes).unwrap();
    assert!(
        image
            .warnings
            .iter()
            .any(|warning| warning.contains("compressed DWARF"))
    );
}

#[test]
fn deterministic_mutated_headers_and_short_bytes_never_panic() {
    for length in 0..64 {
        let bytes: Vec<u8> = (0..length)
            .map(|i| ((i * 79 + length) % 256) as u8)
            .collect();
        let _ = BinaryImage::parse(bytes);
    }
    let original = elf_with_dwarf();
    for index in (0..original.len()).step_by(13) {
        let mut bytes = original.clone();
        bytes[index] ^= 0xff;
        if let Ok(image) = BinaryImage::parse(bytes) {
            let _ = image.disassemble(0x100, 24, 24);
            let _ = image.source_at_offset(0x100);
        }
    }
}

fn pe() -> Vec<u8> {
    let mut bytes = vec![0; 0x220];
    bytes[..2].copy_from_slice(b"MZ");
    put32(&mut bytes, 0x3c, 0x80);
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    put16(&mut bytes, 0x84, 0x8664);
    put16(&mut bytes, 0x86, 1);
    put16(&mut bytes, 0x94, 0xf0);
    put16(&mut bytes, 0x96, 2);
    put16(&mut bytes, 0x98, 0x20b);
    put32(&mut bytes, 0xa8, 0x1000);
    put64(&mut bytes, 0xb0, 0x140000000);
    put32(&mut bytes, 0xb8, 0x1000);
    put32(&mut bytes, 0xbc, 0x200);
    bytes[0x188..0x18d].copy_from_slice(b".text");
    put32(&mut bytes, 0x190, 4); // virtual size excludes raw padding
    put32(&mut bytes, 0x194, 0x1000);
    put32(&mut bytes, 0x198, 0x20);
    put32(&mut bytes, 0x19c, 0x200);
    put32(&mut bytes, 0x1ac, 0x60000020);
    bytes[0x200..0x204].copy_from_slice(&[0x31, 0xc0, 0xc3, 0x90]);
    bytes
}

#[test]
fn pe_addresses_include_image_base_without_mapping_raw_padding() {
    let image = BinaryImage::parse(pe()).unwrap();
    assert_eq!(image.format, BinaryFormat::Pe);
    assert_eq!(image.architecture, Architecture::X86_64);
    assert_eq!(image.entry, Some(0x140001000));
    assert_eq!(image.offset_to_address(0x200), Some(0x140001000));
    assert_eq!(image.address_to_offset(0x140001003), Some(0x203));
    assert_eq!(image.offset_to_address(0x204), None);
    assert_eq!(image.disassemble(0x200, 0x20, 10).unwrap().len(), 3);
    let mut bytes = pe();
    put64(&mut bytes, 0xb0, u64::MAX);
    assert!(BinaryImage::parse(bytes).is_err());
}

#[test]
fn macho64_sections_map_and_disassemble_without_loading_binary() {
    let mut bytes = vec![0u8; 0x204];
    put32(&mut bytes, 0, 0xfeedfacf);
    put32(&mut bytes, 4, 0x01000007); // x86-64
    put32(&mut bytes, 8, 3);
    put32(&mut bytes, 12, 2); // MH_EXECUTE
    put32(&mut bytes, 16, 1);
    put32(&mut bytes, 20, 152);
    put32(&mut bytes, 32, 0x19); // LC_SEGMENT_64
    put32(&mut bytes, 36, 152);
    bytes[40..46].copy_from_slice(b"__TEXT");
    put64(&mut bytes, 56, 0x100000000);
    put64(&mut bytes, 64, 0x1000);
    put64(&mut bytes, 72, 0);
    put64(&mut bytes, 80, 0x204);
    put32(&mut bytes, 88, 7);
    put32(&mut bytes, 92, 5);
    put32(&mut bytes, 96, 1);
    bytes[104..110].copy_from_slice(b"__text");
    bytes[120..126].copy_from_slice(b"__TEXT");
    put64(&mut bytes, 136, 0x100000200);
    put64(&mut bytes, 144, 4);
    put32(&mut bytes, 152, 0x200);
    put32(&mut bytes, 168, 0x80000400);
    bytes[0x200..0x204].copy_from_slice(&[0x31, 0xc0, 0xc3, 0x90]);
    let image = BinaryImage::parse(bytes).unwrap();
    assert_eq!(image.format, BinaryFormat::MachO);
    assert_eq!(image.offset_to_address(0x200), Some(0x100000200));
    assert!(image.sections[0].executable);
    assert_eq!(
        image.disassemble(0x200, 4, 10).unwrap()[0].text,
        "xor eax,eax"
    );
    assert!(
        BinaryImage::parse(vec![0xca, 0xfe, 0xba, 0xbe])
            .unwrap_err()
            .contains("fat Mach-O")
    );
}
