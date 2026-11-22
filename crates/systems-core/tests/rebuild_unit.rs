//! Independent compiler fixture for bounded relocatable-object ingestion.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use ale_systems_core::rebuild::{RebuildErrorCode, RebuildLimits, RelocatableUnit};
use object::{Object, ObjectSection};
use std::{
    fs,
    process::Command,
    sync::atomic::AtomicBool,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(std::path::PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn compile_fixture(source_text: &str, extra_flags: &[&str]) -> Vec<u8> {
    let root = std::env::temp_dir().join(format!(
        "ale-unit-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let root = Fixture(root);
    let source = root.0.join("replacement.c");
    let object = root.0.join("replacement.o");
    fs::write(&source, source_text).unwrap();
    let result = Command::new("/usr/bin/cc")
        // Simulate a CET-enabled host default; the profile must override it.
        .arg("-fcf-protection=full")
        .args([
            "-c",
            "-O1",
            "-ffreestanding",
            "-fno-builtin",
            "-fno-stack-protector",
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            "-fcf-protection=none",
            "-fno-jump-tables",
            "-ffunction-sections",
            "-fdata-sections",
            "-fpie",
            "-fno-optimize-sibling-calls",
        ])
        .args(extra_flags)
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .expect("a C compiler is required for this fixture");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    fs::read(object).unwrap()
}
#[test]
fn compiler_object_with_calls_data_and_readonly_sections_is_admitted() {
    let bytes = compile_fixture(
        concat!(
            "extern int helper(int); extern int counter; ",
            "static const int table[4]={3,7,11,19}; ",
            "int example(int x){ counter+=table[x&3]; ",
            "return helper(counter)+table[(x+1)&3]; }"
        ),
        &[],
    );
    RelocatableUnit::parse(
        &bytes,
        "example",
        &RebuildLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
}

#[test]
fn compiler_cet_property_note_remains_unsupported() {
    let bytes = compile_fixture(
        "int example(int x){return x+1;}",
        &["-fcf-protection=full"],
    );
    let object = object::File::parse(bytes.as_slice()).unwrap();
    let note = object
        .section_by_name(".note.gnu.property")
        .expect("CET fixture must contain a GNU property note");
    assert_eq!(note.kind(), object::SectionKind::Note);
    assert!(matches!(
        note.flags(),
        object::SectionFlags::Elf { sh_flags: 2 }
    ));
    assert!(note.size() > 0);
    let error = RelocatableUnit::parse(
        &bytes,
        "example",
        &RebuildLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(error.code, RebuildErrorCode::UnsupportedObject);
    for detail in [".note.gnu.property", "type=0x7", "flags=0x2"] {
        assert!(error.detail.contains(detail), "{error:?}");
    }
}
