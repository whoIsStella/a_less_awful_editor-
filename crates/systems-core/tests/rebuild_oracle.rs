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
    run_owned_args(path, &[])
}
fn run_owned_args(path: &Path, args: &[&str]) -> (Output, String) {
    let mut child = Command::new(path)
        .args(args)
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
fn build(label: &str, pie: bool) -> (Fixture, Vec<u8>, Vec<u8>, u64, u64) {
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
            if pie { "-pie" } else { "-no-pie" },
            if pie { "-fPIE" } else { "-fno-pie" },
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
            "-x",
            "c",
            "-std=c11",
            "-O2",
            "-m64",
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
            "-g0",
            "-Wall",
            "-Werror",
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
    let (root, input, output, entry, body) = build("growth", false);
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
    let (root, _, output, _, _) = build("auxv", false);
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
            "-x",
            "c",
            "-std=c11",
            "-O2",
            "-m64",
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
            "-g0",
            "-Wall",
            "-Werror",
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

#[test]
fn et_exec_resolves_helper_existing_data_and_readonly_constants() {
    exercise_calls_data(false);
}
#[test]
fn pie_resolves_helper_existing_data_and_readonly_constants() {
    exercise_calls_data(true);
}
fn exercise_calls_data(pie: bool) {
    use ale_systems_core::{AnalysisLimits, Architecture, EdgeKind};
    let root = Fixture::new();
    let src = root.0.join("original.c");
    let orig = root.0.join("original.elf");
    let edit = root.0.join("replacement.c");
    let object = root.0.join("replacement.o");
    let original = r#"#include <stdio.h>
#include <stdlib.h>
__attribute__((visibility("hidden"))) int counter=11;
__attribute__((noinline,visibility("hidden"))) int helper(int x){return 3*x+7;}
__attribute__((noinline)) int example(int x){return x*x+17;}
int main(int argc,char **argv){int x=argc>1?atoi(argv[1]):0; int y=example(x); printf("result=%d counter=%d\n",y,counter); return 0;}
"#;
    let replacement = r#"extern __attribute__((visibility("hidden"))) int helper(int);
extern __attribute__((visibility("hidden"))) int counter;
static const int table[8]={3,7,11,19,29,43,59,83};
int example(int x){unsigned i=(unsigned)x&7; counter+=table[i]; return helper(counter)+table[(i+3)&7]+x;}
"#;
    fs::write(&src, original).unwrap();
    fs::write(&edit, replacement).unwrap();
    let mode = if pie { "-pie" } else { "-no-pie" };
    let pic = if pie { "-fPIE" } else { "-fno-pie" };
    run_tool(
        "/usr/bin/cc",
        &[
            "-g",
            "-O1",
            mode,
            pic,
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            "-fcf-protection=none",
            src.to_str().unwrap(),
            "-o",
            orig.to_str().unwrap(),
        ],
    );
    run_tool(
        "/usr/bin/cc",
        &[
            "-c",
            "-x",
            "c",
            "-std=c11",
            "-O2",
            "-m64",
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
            "-g0",
            "-Wall",
            "-Werror",
            edit.to_str().unwrap(),
            "-o",
            object.to_str().unwrap(),
        ],
    );
    let objdump = run_tool(
        "/usr/bin/readelf",
        &["-rW", "-SW", object.to_str().unwrap()],
    );
    let object_report = String::from_utf8_lossy(&objdump.stdout);
    assert!(
        object_report.contains("R_X86_64_PC32")
            && object_report.contains("R_X86_64_PLT32")
            && object_report.contains(".rodata")
            && object_report.contains("counter")
            && object_report.contains("helper")
    );
    let input = fs::read(&orig).unwrap();
    let image = BinaryImage::parse(input.clone()).unwrap();
    let symbol = |name: &str| {
        image
            .symbols
            .iter()
            .find(|s| s.name == name)
            .unwrap()
            .address
    };
    let entry = symbol("example");
    let helper = symbol("helper");
    let counter = symbol("counter");
    let counter_offset = image.address_to_offset(counter).unwrap();
    let site = ReplacementSite::discover(&image, entry).unwrap();
    let helper_offset = image.address_to_offset(helper).unwrap();
    let entry_source = image
        .source_at_offset(site.offset)
        .expect("fixture entry DWARF")
        .clone();
    let helper_source = image
        .source_at_offset(helper_offset)
        .expect("fixture helper DWARF")
        .clone();
    assert!(!image.source_locations.is_empty());
    let c = AtomicBool::new(false);
    let unit = RelocatableUnit::parse(
        &fs::read(&object).unwrap(),
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
    assert!(plan.summary().rodata_bytes > 0);
    let body = plan.summary().body_address;
    let output = plan.apply_to(&image).unwrap().bytes().to_vec();
    assert_eq!(
        &output[counter_offset..counter_offset + 4],
        &input[counter_offset..counter_offset + 4]
    );
    let reopened = BinaryImage::parse(output.clone()).unwrap();
    assert_eq!(reopened.rewrite_locations().len(), 1);
    assert!(reopened.source_at_offset(site.offset).is_none());
    assert_eq!(
        reopened.source_at_offset(helper_offset),
        Some(&helper_source)
    );
    assert!(
        entry_source.address < entry + site.capacity as u64 && entry_source.end_address > entry
    );
    let note_offset = reopened
        .sections
        .iter()
        .find(|s| s.name == ".note.ale.rewrite")
        .unwrap()
        .file_offset;
    for offset in [note_offset, note_offset + 16, note_offset + 36, site.offset] {
        let mut corrupt = output.clone();
        corrupt[offset] ^= 0x40;
        let corrupt = BinaryImage::parse(corrupt).unwrap();
        assert!(corrupt.rewrite_locations().is_empty() && corrupt.source_locations.is_empty());
        assert!(
            corrupt
                .warnings
                .iter()
                .any(|w| w.contains("rewrite metadata"))
        );
        assert!(
            plan_replacement(
                &corrupt,
                &site,
                &unit,
                RebuildOptions {
                    mode: PlacementMode::Relocated,
                    limits: RebuildLimits::default()
                },
                &c
            )
            .is_err()
        );
    }
    assert!(
        plan_replacement(
            &reopened,
            &site,
            &unit,
            RebuildOptions {
                mode: PlacementMode::Relocated,
                limits: RebuildLimits::default()
            },
            &c
        )
        .is_err()
    );
    let body_symbol = reopened
        .symbols
        .iter()
        .find(|s| s.name == "__ale_body_example")
        .unwrap();
    assert_eq!(
        (body_symbol.address, body_symbol.size),
        (body, plan.summary().body_span.len() as u64)
    );
    assert!(
        reopened
            .sections
            .iter()
            .any(|s| s.name.starts_with(".ale.rodata.") && s.file_size > 0 && !s.executable)
    );
    let analysis = reopened
        .analyze(&Architecture::X86_64, AnalysisLimits::default(), &c)
        .unwrap();
    let graph = analysis.call_graph(&c).unwrap();
    assert!(
        analysis
            .functions
            .iter()
            .filter(|f| f.address == entry)
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.edges)
            .any(|e| e.kind == EdgeKind::Jump && e.target_address == Some(body))
    );
    assert!(
        graph
            .sites
            .iter()
            .any(|s| analysis.functions[s.caller].address == body
                && s.target_address == Some(helper)
                && s.callee.is_some())
    );
    let call = graph
        .sites
        .iter()
        .find(|s| analysis.functions[s.caller].address == body && s.target_address == Some(helper))
        .unwrap();
    assert_eq!(reopened.offset_to_address(call.offset), Some(call.address));
    assert!(
        !graph.sites.iter().any(
            |s| analysis.functions[s.caller].address == entry && s.target_address == Some(body)
        )
    );
    let export = root.0.join("export.elf");
    fs::write(&export, &output).unwrap();
    fs::set_permissions(&export, fs::Permissions::from_mode(0o700)).unwrap();
    let table = [3, 7, 11, 19, 29, 43, 59, 83];
    let mut observations = String::new();
    for x in [-100i32, -1, 0, 1, 4, 31, 100] {
        let i = (x as u32 & 7) as usize;
        let count = 11 + table[i];
        let expected = 3 * count + 7 + table[(i + 3) & 7] + x;
        let arg = x.to_string();
        let (result, _) = run_owned_args(&export, &[&arg]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&result.stdout).trim(),
            format!("result={expected} counter={count}")
        );
        observations.push_str(&format!(
            "input={x} {}",
            String::from_utf8_lossy(&result.stdout)
        ));
    }
    assert_eq!(fs::read(&orig).unwrap(), input);
    save(
        &format!("{}-runtime.txt", if pie { "pie-calls" } else { "calls" }),
        observations.as_bytes(),
    );
    save(
        &format!("{}-original.c", if pie { "pie-calls" } else { "calls" }),
        original.as_bytes(),
    );
    save(
        &format!("{}-replacement.c", if pie { "pie-calls" } else { "calls" }),
        replacement.as_bytes(),
    );
    save(
        &format!("{}-original.elf", if pie { "pie-calls" } else { "calls" }),
        &input,
    );
    save(
        &format!("{}-replacement.o", if pie { "pie-calls" } else { "calls" }),
        &fs::read(object).unwrap(),
    );
    save(
        &format!("{}-export.elf", if pie { "pie-calls" } else { "calls" }),
        &output,
    );
    save(
        &format!(
            "{}-object-readelf.txt",
            if pie { "pie-calls" } else { "calls" }
        ),
        &objdump.stdout,
    );
}

#[test]
fn et_exec_self_recursion_calls_new_body() {
    let root = Fixture::new();
    let src = root.0.join("recursive-original.c");
    let orig = root.0.join("recursive-original.elf");
    let edit = root.0.join("recursive-edit.c");
    let obj = root.0.join("recursive-edit.o");
    let source = "__attribute__((noinline)) int example(int x){return x*x+17;} int main(void){return example(4);}\n";
    let replacement =
        "int example(int x){if(x<=0)return 4; volatile int v=example(x-1); return v+3;}\n";
    fs::write(&src, source).unwrap();
    fs::write(&edit, replacement).unwrap();
    run_tool(
        "/usr/bin/cc",
        &[
            "-O1",
            "-no-pie",
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            "-fcf-protection=none",
            src.to_str().unwrap(),
            "-o",
            orig.to_str().unwrap(),
        ],
    );
    run_tool(
        "/usr/bin/cc",
        &[
            "-c",
            "-x",
            "c",
            "-std=c11",
            "-O2",
            "-m64",
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
            "-g0",
            "-Wall",
            "-Werror",
            edit.to_str().unwrap(),
            "-o",
            obj.to_str().unwrap(),
        ],
    );
    let report = run_tool("/usr/bin/readelf", &["-rW", obj.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&report.stdout).contains("example"));
    let original = fs::read(&orig).unwrap();
    let image = BinaryImage::parse(original.clone()).unwrap();
    let entry = image
        .symbols
        .iter()
        .find(|s| s.name == "example")
        .unwrap()
        .address;
    let site = ReplacementSite::discover(&image, entry).unwrap();
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
    assert!(
        plan.summary()
            .relocations
            .iter()
            .any(|r| r.symbol == "example" && r.target_address == plan.summary().body_address)
    );
    let output = plan.apply_to(&image).unwrap().bytes().to_vec();
    let export = root.0.join("recursive-export.elf");
    fs::write(&export, &output).unwrap();
    fs::set_permissions(&export, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(run_owned(&orig).0.status.code(), Some(33));
    assert_eq!(run_owned(&export).0.status.code(), Some(16));
    assert_eq!(fs::read(&orig).unwrap(), original);
    save("recursion-original.c", source.as_bytes());
    save("recursion-edit.c", replacement.as_bytes());
    save("recursion-original.elf", &original);
    save("recursion-export.elf", &output);
    save("recursion-relocations.txt", &report.stdout);
}

#[test]
fn et_exec_rejects_bss_dynamic_relocations_and_ambiguous_data() {
    use ale_systems_core::rebuild::RebuildErrorCode;
    let root = Fixture::new();
    let cases = [
        (
            "bss",
            "__attribute__((visibility(\"hidden\"))) int counter;",
            None,
            "extern int counter; int example(int x){return counter+x;}",
            RebuildErrorCode::UnsupportedObject,
        ),
        (
            "dynamic",
            "#include <stdio.h>\n__attribute__((visibility(\"hidden\"))) int (*counter)(const char*)=puts;",
            None,
            "extern int (*counter)(const char*); int example(int x){return (counter!=0)+x;}",
            RebuildErrorCode::UnsupportedObject,
        ),
        (
            "ambiguous",
            "static volatile int counter=3; __attribute__((noinline)) int use_first(void){return counter;}",
            Some(
                "static volatile int counter=7; __attribute__((noinline)) int use_second(void){return counter;}",
            ),
            "extern int counter; int example(int x){return counter+x;}",
            RebuildErrorCode::AmbiguousSymbol,
        ),
    ];
    for (label, declaration, extra, replacement, expected) in cases {
        let source = format!(
            "{declaration}\n__attribute__((noinline)) int example(int x){{return x*x+17;}} int main(void){{return example(3);}}\n"
        );
        let orig_c = root.0.join(format!("{label}.c"));
        let extra_c = root.0.join(format!("{label}-extra.c"));
        let orig = root.0.join(format!("{label}.elf"));
        let edit = root.0.join(format!("{label}-edit.c"));
        let object = root.0.join(format!("{label}.o"));
        fs::write(&orig_c, &source).unwrap();
        fs::write(&edit, replacement).unwrap();
        let mut args = vec![
            "-O1",
            "-no-pie",
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            "-fcf-protection=none",
            orig_c.to_str().unwrap(),
        ];
        if let Some(extra) = extra {
            fs::write(&extra_c, extra).unwrap();
            args.push(extra_c.to_str().unwrap());
        }
        args.extend(["-o", orig.to_str().unwrap()]);
        run_tool("/usr/bin/cc", &args);
        run_tool(
            "/usr/bin/cc",
            &[
                "-c",
                "-x",
                "c",
                "-std=c11",
                "-O2",
                "-m64",
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
                "-g0",
                "-Wall",
                "-Werror",
                edit.to_str().unwrap(),
                "-o",
                object.to_str().unwrap(),
            ],
        );
        let input = fs::read(&orig).unwrap();
        let image = BinaryImage::parse(input.clone()).unwrap();
        let entry = image
            .symbols
            .iter()
            .find(|s| s.name == "example")
            .unwrap()
            .address;
        let site = ReplacementSite::discover(&image, entry).unwrap();
        let c = AtomicBool::new(false);
        let unit = RelocatableUnit::parse(
            &fs::read(&object).unwrap(),
            "example",
            &RebuildLimits::default(),
            &c,
        )
        .unwrap();
        let result = plan_replacement(
            &image,
            &site,
            &unit,
            RebuildOptions {
                mode: PlacementMode::Relocated,
                limits: RebuildLimits::default(),
            },
            &c,
        );
        assert_eq!(result.err().unwrap().code, expected, "{label}");
        assert_eq!(fs::read(&orig).unwrap(), input);
        let report = run_tool("/usr/bin/readelf", &["-rW", "-sW", orig.to_str().unwrap()]);
        if label == "dynamic" {
            let address = image
                .symbols
                .iter()
                .find(|s| s.name == "counter")
                .unwrap()
                .address;
            assert!(String::from_utf8_lossy(&report.stdout).contains(&format!("{address:016x}")));
        }
        save(&format!("negative-{label}-source.c"), source.as_bytes());
        save(&format!("negative-{label}-readelf.txt"), &report.stdout);
    }
}

#[test]
fn pie_auxv_mappings_and_aslr_preserve_relocated_behavior() {
    let (root, _, output, _, body) = build("pie-aslr", true);
    let phoff = u64::from_le_bytes(output[32..40].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(output[56..58].try_into().unwrap()) as usize;
    let ph = &output[phoff..phoff + phnum * 56];
    let file_phdr = u64::from_le_bytes(ph[16..24].try_into().unwrap());
    let expected_hex: String = ph.iter().map(|b| format!("{b:02x}")).collect();
    let aslr = fs::read_to_string("/proc/sys/kernel/randomize_va_space").expect("read ASLR policy");
    assert_ne!(
        aslr.trim(),
        "0",
        "ASLR is disabled; PIE variation cannot be established"
    );
    let mut biases = std::collections::BTreeSet::new();
    let mut samples = String::new();
    let mut all_maps = String::new();
    for index in 0..8 {
        let (result, maps) = run_owned(&root.0.join("export.elf"));
        assert!(result.status.success());
        let (value, phdr, count, entry_size, _) = parse_output(&result.stdout);
        assert_eq!((value, count, entry_size), (197, phnum as u64, 56));
        let text = std::str::from_utf8(&result.stdout).unwrap();
        let actual = text
            .split_whitespace()
            .find_map(|v| v.strip_prefix("phbytes="))
            .unwrap();
        assert_eq!(actual, expected_hex);
        let bias = phdr.checked_sub(file_phdr).expect("load bias");
        biases.insert(bias);
        let entries: Vec<_> = maps
            .lines()
            .filter(|line| line.contains("export.elf"))
            .collect();
        assert!(!entries.is_empty());
        assert!(
            entries
                .iter()
                .any(|line| line.starts_with(&format!("{:x}-", body + bias))
                    && line.split_whitespace().nth(1) == Some("r-xp"))
        );
        assert!(
            entries
                .iter()
                .any(|line| line.starts_with(&format!("{phdr:x}-"))
                    && line.split_whitespace().nth(1) == Some("r--p"))
        );
        assert!(entries.iter().all(|line| {
            !line
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .starts_with("rwx")
        }));
        samples.push_str(&format!("sample={index} bias={bias:#x} {text}"));
        all_maps.push_str(&format!("sample={index}\n{maps}"));
    }
    assert!(biases.len() > 1, "host did not exhibit ASLR variation");
    save("pie-aslr-samples.txt", samples.as_bytes());
    save("pie-aslr-maps.txt", all_maps.as_bytes());
    save("pie-aslr-policy.txt", aslr.as_bytes());
}

#[test]
fn pie_rejects_dsos_static_pie_and_unsupported_relocations() {
    use ale_systems_core::rebuild::RebuildErrorCode;
    let root = Fixture::new();
    let src = root.0.join("source.c");
    let edit = root.0.join("edit.c");
    let source = "__attribute__((noinline)) int example(int x){return x*x+17;} int main(void){return example(3);}\n";
    fs::write(&src, source).unwrap();
    let dso = root.0.join("library.so");
    run_tool(
        "/usr/bin/cc",
        &[
            "-shared",
            "-fPIC",
            "-O1",
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            "-fcf-protection=none",
            src.to_str().unwrap(),
            "-o",
            dso.to_str().unwrap(),
        ],
    );
    let static_src = root.0.join("static.c");
    let static_pie = root.0.join("static-pie.elf");
    fs::write(&static_src,"__attribute__((noinline)) int example(int x){return x*x+17;} void _start(void){long r=example(3); __asm__ volatile(\"syscall\": : \"a\"(60),\"D\"(r):\"rcx\",\"r11\",\"memory\"); __builtin_unreachable();}\n").unwrap();
    run_tool(
        "/usr/bin/cc",
        &[
            "-nostdlib",
            "-static-pie",
            "-O1",
            "-fPIE",
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            "-fcf-protection=none",
            static_src.to_str().unwrap(),
            "-o",
            static_pie.to_str().unwrap(),
        ],
    );
    let pie = root.0.join("main.elf");
    run_tool(
        "/usr/bin/cc",
        &[
            "-pie",
            "-fPIE",
            "-O1",
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            "-fcf-protection=none",
            src.to_str().unwrap(),
            "-o",
            pie.to_str().unwrap(),
        ],
    );
    let c = AtomicBool::new(false);
    let object = root.0.join("edit.o");
    fs::write(
        &edit,
        "int example(int x){volatile int v=x+4;return v*3;}\n",
    )
    .unwrap();
    run_tool(
        "/usr/bin/cc",
        &[
            "-c",
            "-x",
            "c",
            "-std=c11",
            "-O2",
            "-m64",
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
            "-g0",
            "-Wall",
            "-Werror",
            edit.to_str().unwrap(),
            "-o",
            object.to_str().unwrap(),
        ],
    );
    let unit = RelocatableUnit::parse(
        &fs::read(&object).unwrap(),
        "example",
        &RebuildLimits::default(),
        &c,
    )
    .unwrap();
    for (label, path) in [("dso", &dso), ("static-pie", &static_pie)] {
        let input = fs::read(path).unwrap();
        let image = BinaryImage::parse(input.clone()).unwrap();
        let address = image
            .symbols
            .iter()
            .find(|s| s.name == "example")
            .unwrap()
            .address;
        let site = ReplacementSite::discover(&image, address).unwrap();
        let error = plan_replacement(
            &image,
            &site,
            &unit,
            RebuildOptions {
                mode: PlacementMode::Relocated,
                limits: RebuildLimits::default(),
            },
            &c,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, RebuildErrorCode::UnsupportedImage, "{label}");
        assert_eq!(fs::read(path).unwrap(), input);
        let report = run_tool(
            "/usr/bin/readelf",
            &["-h", "-lW", "-dW", path.to_str().unwrap()],
        );
        save(&format!("pie-negative-{label}.txt"), &report.stdout);
    }
    let cases = [
        (
            "absolute",
            "extern int counter; unsigned long example(int x){return (unsigned long)&counter+(unsigned)x;}",
            "-fno-pie",
            "R_X86_64_32",
        ),
        (
            "got",
            "extern int counter; int example(int x){return counter+x;}",
            "-fPIC",
            "GOTPCREL",
        ),
        (
            "tls",
            "extern __thread int counter; int example(int x){return counter+x;}",
            "-fPIC",
            "TLS",
        ),
    ];
    for (label, source, flag, expected_relocation) in cases {
        fs::write(&edit, source).unwrap();
        run_tool(
            "/usr/bin/cc",
            &[
                "-c",
                "-x",
                "c",
                "-std=c11",
                "-O2",
                "-m64",
                "-ffreestanding",
                "-fno-builtin",
                "-fno-stack-protector",
                "-fno-asynchronous-unwind-tables",
                "-fno-unwind-tables",
                "-fno-jump-tables",
                "-ffunction-sections",
                "-fdata-sections",
                flag,
                "-fno-optimize-sibling-calls",
                "-g0",
                "-Wall",
                "-Werror",
                edit.to_str().unwrap(),
                "-o",
                object.to_str().unwrap(),
            ],
        );
        let report = run_tool("/usr/bin/readelf", &["-rW", object.to_str().unwrap()]);
        assert!(
            String::from_utf8_lossy(&report.stdout).contains(expected_relocation),
            "{label}"
        );
        assert!(
            RelocatableUnit::parse(
                &fs::read(&object).unwrap(),
                "example",
                &RebuildLimits::default(),
                &c
            )
            .is_err(),
            "{label}"
        );
        save(&format!("pie-negative-{label}.txt"), &report.stdout);
    }
    fs::write(
        &edit,
        "extern int imported(int); int example(int x){return imported(x)+1;}\n",
    )
    .unwrap();
    run_tool(
        "/usr/bin/cc",
        &[
            "-c",
            "-x",
            "c",
            "-std=c11",
            "-O2",
            "-m64",
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
            "-g0",
            "-Wall",
            "-Werror",
            edit.to_str().unwrap(),
            "-o",
            object.to_str().unwrap(),
        ],
    );
    let report = run_tool("/usr/bin/readelf", &["-rW", object.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&report.stdout).contains("R_X86_64_PLT32"));
    let unit = RelocatableUnit::parse(
        &fs::read(&object).unwrap(),
        "example",
        &RebuildLimits::default(),
        &c,
    )
    .unwrap();
    let input = fs::read(&pie).unwrap();
    let image = BinaryImage::parse(input.clone()).unwrap();
    let address = image
        .symbols
        .iter()
        .find(|s| s.name == "example")
        .unwrap()
        .address;
    let site = ReplacementSite::discover(&image, address).unwrap();
    let error = plan_replacement(
        &image,
        &site,
        &unit,
        RebuildOptions {
            mode: PlacementMode::Relocated,
            limits: RebuildLimits::default(),
        },
        &c,
    )
    .err()
    .unwrap();
    assert_eq!(error.code, RebuildErrorCode::UndefinedSymbol);
    assert_eq!(fs::read(&pie).unwrap(), input);
    save("pie-negative-import.txt", &report.stdout);
}
