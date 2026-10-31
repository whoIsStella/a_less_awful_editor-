//! Independent compiler fixture for bounded relocatable-object ingestion.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use ale_systems_core::rebuild::{RebuildLimits, RelocatableUnit};
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
#[test]
fn compiler_object_with_calls_data_and_readonly_sections_is_admitted() {
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
    fs::write(&source,"extern int helper(int); extern int counter; static const int table[4]={3,7,11,19}; int example(int x){ counter+=table[x&3]; return helper(counter)+table[(x+1)&3]; }").unwrap();
    let result = Command::new("/usr/bin/cc")
        .args([
            "-c",
            "-O1",
            "-ffreestanding",
            "-fno-builtin",
            "-fno-stack-protector",
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            "-fno-jump-tables",
            "-ffunction-sections",
            "-fdata-sections",
            "-fpie",
            "-fno-optimize-sibling-calls",
        ])
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
    let bytes = fs::read(object).unwrap();
    RelocatableUnit::parse(
        &bytes,
        "example",
        &RebuildLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
}
