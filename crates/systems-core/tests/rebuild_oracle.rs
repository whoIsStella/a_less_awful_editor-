//! Linux loader and independent binutils checks for owned rewrite fixtures.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use ale_systems_core::{
    BinaryImage,
    rebuild::{PlacementMode, RebuildLimits, RebuildOptions, RelocatableUnit, plan_replacement},
    recompile::ReplacementSite,
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ale-rebuild-oracle-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn run_tool(program: &str, args: &[&str]) -> Output {
    let out = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("required tool {program}: {e}"));
    assert!(
        out.status.success(),
        "{program}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}
fn save(label: &str, data: &[u8]) {
    if let Some(dir) = std::env::var_os("ALE_REBUILD_EVIDENCE") {
        let path = PathBuf::from(dir).join(label);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("create {}: {e}", path.display()));
        file.write_all(data).unwrap();
    }
}
fn run_owned(path: &Path) -> (Output, String) {
    let mut child = Command::new(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch owned fixture");
    thread::sleep(Duration::from_millis(30));
    let maps = fs::read_to_string(format!("/proc/{}/maps", child.id())).unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("owned fixture exceeded three seconds");
        }
        thread::sleep(Duration::from_millis(10));
    }
    (child.wait_with_output().unwrap(), maps)
}
fn build(label: &str) -> (Fixture, Vec<u8>, Vec<u8>, u64, u64) {
    for tool in ["/usr/bin/cc", "/usr/bin/readelf", "/usr/bin/objdump"] {
        assert!(Path::new(tool).is_file(), "required tool missing: {tool}");
    }
    let root = Fixture::new();
    let orig_c = root.0.join("original.c");
    let orig = root.0.join("original.elf");
    let edit_c = root.0.join("replacement.c");
    let obj = root.0.join("replacement.o");
    let source = r#"#include <stdio.h>
#include <stdint.h>
#include <sys/auxv.h>
#include <unistd.h>
__attribute__((noinline)) int example(int x) { return x*x+17; }
int main(void) {
    unsigned long p=getauxval(AT_PHDR), n=getauxval(AT_PHNUM), e=getauxval(AT_PHENT);
    const unsigned char *b=(const unsigned char*)p;
    uint64_t hash=14695981039346656037ULL;
    for(unsigned long i=0;i<n*e;i++) { hash^=b[i]; hash*=1099511628211ULL; }
    printf("value=%d phdr=%lx phnum=%lu phent=%lu hash=%llx phbytes=",example(7),p,n,e,(unsigned long long)hash);
    for(unsigned long i=0;i<n*e;i++) printf("%02x",b[i]);
    putchar('\n'); fflush(stdout); usleep(200000); return 0;
}
"#;
    let replacement = "int example(int x) { volatile int v=x*7+100; return (v^0x35)+(v>>2); }\n";
    fs::write(&orig_c, source).unwrap();
    fs::write(&edit_c, replacement).unwrap();
    let orig_s = orig.to_str().unwrap();
    let orig_c_s = orig_c.to_str().unwrap();
    let obj_s = obj.to_str().unwrap();
    let edit_s = edit_c.to_str().unwrap();
    run_tool(
        "/usr/bin/cc",
        &[
            "-O1",
            "-no-pie",
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            "-fcf-protection=none",
            orig_c_s,
            "-o",
            orig_s,
        ],
    );
    run_tool(
        "/usr/bin/cc",
        &[
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
            edit_s,
            "-o",
            obj_s,
        ],
    );
    let input = fs::read(&orig).unwrap();
    let image = BinaryImage::parse(input.clone()).unwrap();
    let address = image
        .symbols
        .iter()
        .find(|s| s.name == "example")
        .expect("fixture symbol")
        .address;
    let site = ReplacementSite::discover(&image, address).unwrap();
    let object = fs::read(&obj).unwrap();
    let cancel = AtomicBool::new(false);
    let unit =
        RelocatableUnit::parse(&object, "example", &RebuildLimits::default(), &cancel).unwrap();
    let plan = plan_replacement(
        &image,
        &site,
        &unit,
        RebuildOptions {
            mode: PlacementMode::Relocated,
            limits: RebuildLimits::default(),
        },
        &cancel,
    )
    .unwrap();
    assert!(
        plan.summary().body_span.len() > site.capacity,
        "fixture must prove function growth"
    );
    let output = plan.apply_to(&image).unwrap().bytes().to_vec();
    assert_eq!(fs::read(&orig).unwrap(), input);
    let allow = [
        0x20..0x28,
        0x28..0x30,
        0x38..0x3a,
        0x3c..0x3e,
        site.offset..site.offset + site.capacity,
    ];
    for (i, (&old, &new)) in input.iter().zip(&output).enumerate() {
        if old != new {
            assert!(
                allow.iter().any(|r| r.contains(&i)),
                "unexpected original byte {i:#x}"
            );
        }
    }
    let old_headers = program_headers(&input);
    let new_headers = program_headers(&output);
    let retained: Vec<_> = old_headers
        .into_iter()
        .filter(|h| u32::from_le_bytes(h[..4].try_into().unwrap()) != 6)
        .collect();
    assert_eq!(&new_headers[1..new_headers.len() - 2], retained.as_slice());
    let displacement =
        i32::from_le_bytes(output[site.offset + 1..site.offset + 5].try_into().unwrap());
    assert_eq!(output[site.offset], 0xe9);
    assert_eq!(
        (i128::from(site.address) + 5 + i128::from(displacement)) as u64,
        plan.summary().body_address
    );
    save(&format!("{label}-original.c"), source.as_bytes());
    save(&format!("{label}-replacement.c"), replacement.as_bytes());
    save(&format!("{label}-original.elf"), &input);
    save(&format!("{label}-replacement.o"), &object);
    save(&format!("{label}-export.elf"), &output);
    let export = root.0.join("export.elf");
    fs::write(&export, &output).unwrap();
    fs::set_permissions(&export, fs::Permissions::from_mode(0o700)).unwrap();
    let export_s = export.to_str().unwrap();
    let readelf = run_tool("/usr/bin/readelf", &["-h", "-lW", "-SW", "-sW", export_s]);
    save(&format!("{label}-readelf.txt"), &readelf.stdout);
    let objdump = run_tool("/usr/bin/objdump", &["-d", export_s]);
    save(&format!("{label}-objdump.txt"), &objdump.stdout);
    let relocs = run_tool("/usr/bin/readelf", &["-rW", obj_s]);
    save(&format!("{label}-object-relocations.txt"), &relocs.stdout);
    let output_text = String::from_utf8_lossy(&readelf.stdout);
    assert!(output_text.contains(&format!("{:016x}", plan.summary().body_address)));
    assert!(String::from_utf8_lossy(&objdump.stdout).contains(&format!(
        "{:x} <__ale_body_example>",
        plan.summary().body_address
    )));
    assert!(
        output_text.contains("__ale_body_example")
            && output_text.contains(".ale.text.example")
            && output_text.contains(".note.ale.rewrite")
    );
    let (baseline, _) = run_owned(&orig);
    assert!(baseline.status.success());
    assert!(String::from_utf8_lossy(&baseline.stdout).contains("value=66 "));
    (
        root,
        input,
        output,
        plan.summary().entry_address,
        plan.summary().body_address,
    )
}
fn program_headers(bytes: &[u8]) -> Vec<[u8; 56]> {
    let offset = u64::from_le_bytes(bytes[32..40].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(bytes[56..58].try_into().unwrap()) as usize;
    (0..count)
        .map(|i| {
            bytes[offset + i * 56..offset + (i + 1) * 56]
                .try_into()
                .unwrap()
        })
        .collect()
}
fn parse_output(stdout: &[u8]) -> (u64, u64, u64, u64, u64) {
    let text = std::str::from_utf8(stdout).unwrap();
    let mut values = std::collections::BTreeMap::new();
    for field in text.split_whitespace() {
        let (k, v) = field.split_once('=').unwrap();
        values.insert(k, v);
    }
    (
        values["value"].parse().unwrap(),
        u64::from_str_radix(values["phdr"], 16).unwrap(),
        values["phnum"].parse().unwrap(),
        values["phent"].parse().unwrap(),
        u64::from_str_radix(values["hash"], 16).unwrap(),
    )
}
fn run_export(root: &Fixture, label: &str) -> (Output, String) {
    let (out, maps) = run_owned(&root.0.join("export.elf"));
    save(&format!("{label}-stdout.txt"), &out.stdout);
    save(&format!("{label}-stderr.txt"), &out.stderr);
    save(&format!("{label}-maps.txt"), maps.as_bytes());
    (out, maps)
}
#[test]
fn grown_et_exec_loads_through_original_entry() {
    let (root, input, output, entry, body) = build("growth");
    let (result, maps) = run_export(&root, "growth");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(parse_output(&result.stdout).0, 197);
    assert_ne!(entry, body);
    assert_ne!(input, output);
    let entries: Vec<_> = maps
        .lines()
        .filter(|line| line.contains("export.elf"))
        .collect();
    assert!(
        !entries.is_empty(),
        "controlled process mappings were unavailable"
    );
    assert!(
        entries
            .iter()
            .any(|line| line.starts_with(&format!("{body:08x}-"))
                && line.split_whitespace().nth(1) == Some("r-xp"))
    );
    let phdr = parse_output(&result.stdout).1;
    assert!(
        entries
            .iter()
            .any(|line| line.starts_with(&format!("{phdr:08x}-"))
                && line.split_whitespace().nth(1) == Some("r--p"))
    );
    for line in entries {
        assert!(
            !line
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .starts_with("rwx")
        );
    }
}
#[test]
fn mapped_program_headers_match_auxv() {
    let (root, _, output, _, _) = build("auxv");
    let (result, _) = run_export(&root, "auxv");
    assert!(result.status.success());
    let (_, phdr, phnum, phent, hash) = parse_output(&result.stdout);
    let phoff = u64::from_le_bytes(output[32..40].try_into().unwrap()) as usize;
    let expected_num = u16::from_le_bytes(output[56..58].try_into().unwrap()) as u64;
    assert_eq!(phnum, expected_num);
    assert_eq!(phent, 56);
    let ph = &output[phoff..phoff + phnum as usize * 56];
    let expected_hash = ph.iter().fold(14695981039346656037u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(1099511628211)
    });
    assert_eq!(hash, expected_hash);
    let text = std::str::from_utf8(&result.stdout).unwrap();
    let actual_hex = text
        .split_whitespace()
        .find_map(|field| field.strip_prefix("phbytes="))
        .expect("mapped program header bytes");
    let expected_hex: String = ph.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(actual_hex, expected_hex);
    assert_eq!(phdr, u64::from_le_bytes(ph[16..24].try_into().unwrap()));
}

#[test]
fn freestanding_static_et_exec_loads_through_original_entry() {
    let root = Fixture::new();
    let orig_c = root.0.join("static.c");
    let orig = root.0.join("static.elf");
    let edit_c = root.0.join("replacement.c");
    let obj = root.0.join("replacement.o");
    let source = r#"__attribute__((noinline)) int example(int x) { return x*x+17; }
void _start(void) { long r=example(7); __asm__ volatile("syscall" : : "a"(60), "D"(r) : "rcx", "r11", "memory"); __builtin_unreachable(); }
"#;
    let replacement = "int example(int x) { volatile int v=x*7+100; return (v^0x35)+(v>>2); }\n";
    fs::write(&orig_c, source).unwrap();
    fs::write(&edit_c, replacement).unwrap();
    run_tool(
        "/usr/bin/cc",
        &[
            "-O1",
            "-nostdlib",
            "-static",
            "-no-pie",
            "-fno-pie",
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            "-fcf-protection=none",
            orig_c.to_str().unwrap(),
            "-o",
            orig.to_str().unwrap(),
        ],
    );
    run_tool(
        "/usr/bin/cc",
        &[
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
            edit_c.to_str().unwrap(),
            "-o",
            obj.to_str().unwrap(),
        ],
    );
    let input = fs::read(&orig).unwrap();
    let image = BinaryImage::parse(input.clone()).unwrap();
    let address = image
        .symbols
        .iter()
        .find(|s| s.name == "example")
        .unwrap()
        .address;
    let site = ReplacementSite::discover(&image, address).unwrap();
    let c = AtomicBool::new(false);
    let unit = RelocatableUnit::parse(
        &fs::read(&obj).unwrap(),
        "example",
        &RebuildLimits::default(),
        &c,
    )
    .unwrap();
    let plan = plan_replacement(
        &image,
        &site,
        &unit,
        RebuildOptions {
            mode: PlacementMode::Relocated,
            limits: RebuildLimits::default(),
        },
        &c,
    )
    .unwrap();
    assert!(plan.summary().body_span.len() > site.capacity);
    let output = plan.apply_to(&image).unwrap().bytes().to_vec();
    let export = root.0.join("static-export.elf");
    fs::write(&export, &output).unwrap();
    fs::set_permissions(&export, fs::Permissions::from_mode(0o700)).unwrap();
    let (baseline, _) = run_owned(&orig);
    assert_eq!(baseline.status.code(), Some(66));
    let (result, _) = run_owned(&export);
    assert_eq!(result.status.code(), Some(197));
    assert_eq!(fs::read(&orig).unwrap(), input);
    let readelf = run_tool(
        "/usr/bin/readelf",
        &["-h", "-lW", "-SW", "-sW", export.to_str().unwrap()],
    );
    let report = String::from_utf8_lossy(&readelf.stdout);
    assert!(!report.contains(" INTERP "));
    assert!(report.contains("__ale_body_example"));
    save("static-original.c", source.as_bytes());
    save("static-original.elf", &input);
    save("static-export.elf", &output);
    save("static-readelf.txt", &readelf.stdout);
}
