//! Trusted host C compiler, never target execution. Run on a background executor.
use ale_systems_core::recompile::ReplacementSite;
use std::{
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
const PRELUDE: &str = "typedef unsigned char byte,uchar,undefined,undefined1,uint1;\ntypedef signed char int1;\ntypedef unsigned short ushort,undefined2,uint2;\ntypedef short int2;\ntypedef unsigned int uint,undefined4,uint4;\ntypedef int int4;\ntypedef unsigned long ulong,undefined8,uint8;\ntypedef long int8;\n";
fn validate(source: &str) -> Result<(), String> {
    if source.len() > 65536 || source.contains('\0') {
        return Err("Replacement C is limited to 64 KiB without NUL bytes.".into());
    }
    if source.contains('#')
        || source.contains("%:")
        || source.contains("??")
        || source
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(|s| matches!(s, "asm" | "__asm" | "__asm__" | "_Pragma"))
    {
        return Err("This C replacement path does not accept preprocessor directives, trigraphs, inline assembly or pragmas. Use the assembly/bytes tools for instruction edits.".into());
    }
    Ok(())
}
pub(crate) fn compile(
    source: &str,
    site: &ReplacementSite,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    if !cfg!(target_os = "linux") {
        return Err("Native C replacement currently requires Linux.".into());
    }
    validate(source)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Compilation cancelled.".into());
    }
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let input = dir.path().join("replacement.c");
    let output = dir.path().join("replacement.o");
    let diagnostics = dir.path().join("compiler.stderr");
    fs::write(&input, format!("{PRELUDE}\n{source}\n")).map_err(|e| e.to_string())?;
    let mut command = Command::new("/usr/bin/cc");
    command
        .current_dir(dir.path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("TMPDIR", dir.path())
        .args([
            "-x",
            "c",
            "-std=c11",
            "-O2",
            "-m64",
            "-ffreestanding",
            "-fno-builtin",
            "-fno-stack-protector",
            "-fno-pic",
            "-fno-pie",
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            "-fno-jump-tables",
            "-ffunction-sections",
            "-fdata-sections",
            "-g0",
            "-Wall",
            "-Werror",
            "-c",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&diagnostics).map_err(|e| e.to_string())?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        unsafe {
            command.pre_exec(|| {
                for (resource, limit) in [
                    (libc::RLIMIT_CPU, 5),
                    (libc::RLIMIT_AS, 1024 * 1024 * 1024),
                    (libc::RLIMIT_FSIZE, 2 * 1024 * 1024),
                    (libc::RLIMIT_CORE, 0),
                    (libc::RLIMIT_NOFILE, 64),
                ] {
                    let bound = libc::rlimit {
                        rlim_cur: limit,
                        rlim_max: limit,
                    };
                    if libc::setrlimit(resource, &bound) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not start /usr/bin/cc: {e}"))?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Ok(None)
                if start.elapsed() < Duration::from_secs(10) && !cancel.load(Ordering::Relaxed) =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            other => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                break Err(if cancel.load(Ordering::Relaxed) {
                    "Compilation cancelled; no preview changed.".into()
                } else {
                    format!("Compiler stopped after timeout/wait failure: {other:?}")
                });
            }
        }
    }?;
    let mut detail = String::new();
    fs::File::open(&diagnostics)
        .map_err(|e| e.to_string())?
        .take(65536)
        .read_to_string(&mut detail)
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("C compilation failed ({status}):\n{detail}"));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Compilation cancelled; no preview changed.".into());
    }
    let bytes = bounded_object(&output)?;
    site.replacement(&bytes)
}
fn bounded_object(path: &Path) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 2 * 1024 * 1024 {
        return Err("Compiler object exceeds 2 MiB.".into());
    }
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
pub(crate) mod tests {
    use super::*;
    use ale_systems_core::{AnalysisLimits, Architecture, BinaryImage};
    pub(crate) fn fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        BinaryImage,
        ReplacementSite,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("fixture.c");
        let binary = dir.path().join("fixture.elf");
        fs::write(&source,"__attribute__((noinline,optimize(\"O0\"))) int roundtrip(int x) { return x + 1; }\nint atoi(const char*); int main(int argc,char**argv) { return roundtrip(argc>1?atoi(argv[1]):4) & 255; }\n").unwrap();
        let result = Command::new("/usr/bin/cc")
            .args(["-O1", "-fno-pie", "-no-pie", "-fno-stack-protector"])
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let image = BinaryImage::parse(fs::read(&binary).unwrap()).unwrap();
        let address = image
            .symbols
            .iter()
            .find(|s| s.name == "roundtrip")
            .unwrap()
            .address;
        let site = ReplacementSite::discover(&image, address).unwrap();
        (dir, binary, image, site)
    }
    fn fixture_exit(path: &Path) -> Option<i32> {
        fixture_exit_with(path, &[])
    }
    fn fixture_exit_with(path: &Path, args: &[&str]) -> Option<i32> {
        let mut child = Command::new(path)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status.code();
            }
            if start.elapsed() > Duration::from_secs(3) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Controlled round-trip fixture exceeded 3 seconds");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn compiled_function_changes_only_its_site_and_runs_in_exported_fixture() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, binary, image, site) = fixture();
        let before = fs::read(&binary).unwrap();
        // Execute only this test's compiler-generated disposable fixture.
        assert_eq!(fixture_exit(&binary), Some(5));
        let replacement = compile(
            "int roundtrip(int x) { return x + 7; }",
            &site,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(replacement.len(), site.capacity);
        let after = image.patched(site.offset, &replacement).unwrap();
        assert_eq!(&before[..site.offset], &after.bytes()[..site.offset]);
        assert_eq!(
            &before[site.offset + site.capacity..],
            &after.bytes()[site.offset + site.capacity..]
        );
        let export = dir.path().join("changed.elf");
        crate::systems_io::export_executable_copy(&export, after.bytes()).unwrap();
        assert_eq!(
            fs::metadata(&export).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(fixture_exit(&export), Some(11));
        for value in [-100i32, -1, 0, 4, 100] {
            let argument = value.to_string();
            assert_eq!(
                fixture_exit_with(&binary, &[&argument]),
                Some((value + 1) & 255)
            );
            assert_eq!(
                fixture_exit_with(&export, &[&argument]),
                Some((value + 7) & 255)
            );
        }
        assert_eq!(fs::read(&binary).unwrap(), before);
        assert!(crate::systems_io::export_copy(&export, after.bytes()).is_err());
    }
    #[test]
    fn compiler_rejects_unsupported_output_and_preserves_original() {
        let (_dir, binary, image, site) = fixture();
        let before = image.bytes().to_vec();
        for source in [
            "int other(int); int roundtrip(int x){return other(x);}",
            "int global=3; int roundtrip(int x){return x;}",
            "int other(int x){return x;}",
            "#include <stdio.h>\nint roundtrip(int x){return x;}",
            "int roundtrip(int x){ __asm__(\"nop\"); return x;}",
            "int roundtrip(int x){ this is invalid; }",
            "int roundtrip(int x){volatile int a[256]; for(int i=0;i<256;i++) a[i]=x+i; return a[255];}",
        ] {
            assert!(
                compile(source, &site, &AtomicBool::new(false)).is_err(),
                "accepted {source}"
            );
        }
        assert!(compile(&"x".repeat(65537), &site, &AtomicBool::new(false)).is_err());
        assert!(
            compile(
                "int roundtrip(int x){return x;}",
                &site,
                &AtomicBool::new(true)
            )
            .is_err()
        );
        assert_eq!(fs::read(binary).unwrap(), before);
        assert!(
            ReplacementSite::discover(&BinaryImage::parse(vec![0x90, 0xc3]).unwrap(), 0).is_err()
        );
        assert!(ReplacementSite::discover(&image, site.address + 1).is_err());
        let source = _dir.path().join("textrel.S");
        let binary = _dir.path().join("textrel.elf");
        fs::write(
            &source,
            r#".text
.globl roundtrip
.type roundtrip,@function
roundtrip:
.quad puts
.size roundtrip,.-roundtrip
.globl main
.type main,@function
main:
xor %eax,%eax
ret
.section .note.GNU-stack,"",@progbits
"#,
        )
        .unwrap();
        let output = Command::new("/usr/bin/cc")
            .args(["-fPIE", "-pie", "-Wl,-z,notext"])
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let textrel = BinaryImage::parse(fs::read(binary).unwrap()).unwrap();
        let address = textrel
            .symbols
            .iter()
            .find(|s| s.name == "roundtrip")
            .unwrap()
            .address;
        assert!(
            ReplacementSite::discover(&textrel, address)
                .unwrap_err()
                .contains("relocation-aware")
        );
    }
    #[test]
    #[ignore = "requires the pinned native worker; run explicitly with ALE_DECOMPILER_TEST"]
    fn actual_native_decompile_edit_compile_export_and_execute() {
        use std::os::unix::fs::PermissionsExt;
        let Some(worker) = std::env::var_os("ALE_DECOMPILER_TEST") else {
            panic!("Set ALE_DECOMPILER_TEST for the native round-trip acceptance test");
        };
        let (dir, binary, image, site) = fixture();
        let analysis = image
            .analyze(
                &Architecture::X86_64,
                AnalysisLimits::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
        let function = analysis
            .functions
            .iter()
            .find(|f| f.address == site.address)
            .unwrap();
        let result = crate::decompiler::decompile(
            Path::new(&worker),
            &image,
            &analysis,
            function,
            &AtomicBool::new(false),
        )
        .unwrap();
        eprintln!("Actual recovered C:\n{}", result.text);
        assert!(result.text.contains(" + 1"));
        let edited = result.text.replacen(" + 1", " + 7", 1);
        let bytes = compile(&edited, &site, &AtomicBool::new(false)).unwrap();
        let changed = image.patched(site.offset, &bytes).unwrap();
        let export = dir.path().join("recovered-edit.elf");
        crate::systems_io::export_executable_copy(&export, changed.bytes()).unwrap();
        assert_eq!(
            fs::metadata(&export).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(fixture_exit(&binary), Some(5));
        assert_eq!(fixture_exit(&export), Some(11));
        for value in [-100i32, -1, 0, 4, 100] {
            let argument = value.to_string();
            assert_eq!(
                fixture_exit_with(&binary, &[&argument]),
                Some((value + 1) & 255)
            );
            assert_eq!(
                fixture_exit_with(&export, &[&argument]),
                Some((value + 7) & 255)
            );
        }
        assert_eq!(fs::read(binary).unwrap(), image.bytes());
    }
}
