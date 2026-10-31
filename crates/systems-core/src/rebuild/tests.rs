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
