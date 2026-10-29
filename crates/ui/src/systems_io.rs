//! Blocking systems-workbench helpers: run on the background executor only.
//! Binary data is never executed. Exports only create new files. These conservative
//! path/metadata checks do not make a transaction against concurrent filesystem
//! writers (in particular, an ancestor directory can be replaced between checks).

use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const MAX_BINARY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ASSEMBLY_SOURCE: usize = 512;
const MAX_INSTRUCTION_BYTES: u64 = 15;
const MAX_DIAGNOSTIC_BYTES: usize = 4096;
const ASSEMBLY_TIMEOUT: Duration = Duration::from_secs(2);
const NASM_PATH: &str = "/usr/bin/nasm";

fn checked_path(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("Choose a file path.".into());
    }
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("Paths containing '..' are unsupported. Choose the full real path.".into());
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("Symbolic links, including parent directories, are unsupported. Choose the real path.".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(path)
}

fn same_version(before: &Metadata, after: &Metadata) -> bool {
    let unchanged = before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
        && before.permissions() == after.permissions()
        && after.is_file();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        unchanged
            && before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec()
    }
    #[cfg(not(unix))]
    unchanged
}

/// Preserve all bytes, including NUL and invalid UTF-8, in a bounded snapshot.
pub(crate) fn read_binary(path: &Path) -> Result<Vec<u8>, String> {
    read_binary_impl(path, || {})
}

fn read_binary_impl(path: &Path, after_read: impl FnOnce()) -> Result<Vec<u8>, String> {
    let path = checked_path(path)?;
    let selected = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !selected.is_file() {
        return Err("Only regular binary files are supported.".into());
    }
    if selected.len() > MAX_BINARY_BYTES {
        return Err("Binary files larger than 64 MiB are not supported yet.".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Do not follow a substituted final symlink or block on a substituted FIFO.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&path).map_err(|error| error.to_string())?;
    let before = file.metadata().map_err(|error| error.to_string())?;
    if !same_version(&selected, &before) {
        return Err("The selected file changed before it could be read. Try again.".into());
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_BINARY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BINARY_BYTES {
        return Err("Binary files larger than 64 MiB are not supported yet.".into());
    }
    after_read();
    let after = file.metadata().map_err(|error| error.to_string())?;
    checked_path(&path)?;
    let current = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !same_version(&before, &after)
        || !same_version(&after, &current)
        || after.len() != bytes.len() as u64
    {
        return Err(
            "The file changed while being read. Try again; the current document is unchanged."
                .into(),
        );
    }
    Ok(bytes)
}

/// Export to a new path without truncating or replacing an existing destination.
/// A directory-sync error means the copy exists but crash durability is uncertain.
pub(crate) fn export_copy(path: &Path, bytes: &[u8]) -> Result<(), String> {
    export_copy_impl(path, bytes, || {})
}

fn export_copy_impl(path: &Path, bytes: &[u8], before_commit: impl FnOnce()) -> Result<(), String> {
    if bytes.len() as u64 > MAX_BINARY_BYTES {
        return Err("Exports larger than 64 MiB are not supported yet.".into());
    }
    let path = checked_path(path)?;
    match fs::symlink_metadata(&path) {
        Ok(_) => return Err("The export destination already exists. Choose a new file name; nothing was overwritten.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let parent = path.parent().ok_or("No parent directory for this path.")?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temporary
        .write_all(bytes)
        .map_err(|error| error.to_string())?;
    temporary.flush().map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    before_commit();
    checked_path(&path)?;
    temporary.persist_noclobber(&path).map_err(|error| {
        format!("Could not create the export ({error}). Choose a new destination; existing files were not overwritten.")
    })?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!("The export was created, but directory sync failed ({error}). Crash durability is uncertain; the source is unchanged.")
        })?;
    Ok(())
}

// A positive list deliberately excludes NASM's data/section directives, labels,
// macros and repetition constructs. More instruction families can be added with
// encoding tests; accepting arbitrary assembler programs is outside this helper.
fn is_supported_mnemonic(mnemonic: &str) -> bool {
    matches!(
        mnemonic,
        "adc"
            | "add"
            | "and"
            | "bsf"
            | "bsr"
            | "bswap"
            | "bt"
            | "btc"
            | "btr"
            | "bts"
            | "call"
            | "cbw"
            | "cdq"
            | "cdqe"
            | "clc"
            | "cld"
            | "cli"
            | "clts"
            | "cmc"
            | "cmp"
            | "cmpsb"
            | "cmpsw"
            | "cmpsd"
            | "cmpsq"
            | "cmpxchg"
            | "cmpxchg8b"
            | "cmpxchg16b"
            | "cpuid"
            | "cqo"
            | "cwd"
            | "cwde"
            | "dec"
            | "div"
            | "emms"
            | "endbr32"
            | "endbr64"
            | "enter"
            | "hlt"
            | "idiv"
            | "imul"
            | "in"
            | "inc"
            | "insb"
            | "insw"
            | "insd"
            | "int"
            | "int3"
            | "into"
            | "invd"
            | "invlpg"
            | "iret"
            | "iretd"
            | "iretq"
            | "jmp"
            | "jcxz"
            | "jecxz"
            | "jrcxz"
            | "lahf"
            | "lea"
            | "leave"
            | "lfence"
            | "lgdt"
            | "lidt"
            | "lldt"
            | "lmsw"
            | "lodsb"
            | "lodsw"
            | "lodsd"
            | "lodsq"
            | "loop"
            | "loope"
            | "loopne"
            | "loopnz"
            | "loopz"
            | "lsl"
            | "ltr"
            | "mfence"
            | "mov"
            | "movabs"
            | "movsb"
            | "movsw"
            | "movsd"
            | "movsq"
            | "movsx"
            | "movsxd"
            | "movzx"
            | "mul"
            | "neg"
            | "nop"
            | "not"
            | "or"
            | "out"
            | "outsb"
            | "outsw"
            | "outsd"
            | "pause"
            | "pop"
            | "popa"
            | "popad"
            | "popf"
            | "popfd"
            | "popfq"
            | "push"
            | "pusha"
            | "pushad"
            | "pushf"
            | "pushfd"
            | "pushfq"
            | "rcl"
            | "rcr"
            | "rdmsr"
            | "rdpmc"
            | "rdtsc"
            | "rdtscp"
            | "ret"
            | "retf"
            | "retn"
            | "rol"
            | "ror"
            | "rsm"
            | "sahf"
            | "sal"
            | "sar"
            | "sbb"
            | "scasb"
            | "scasw"
            | "scasd"
            | "scasq"
            | "sfence"
            | "sgdt"
            | "shl"
            | "shld"
            | "shr"
            | "shrd"
            | "sidt"
            | "sldt"
            | "smsw"
            | "stc"
            | "std"
            | "sti"
            | "stosb"
            | "stosw"
            | "stosd"
            | "stosq"
            | "str"
            | "sub"
            | "swapgs"
            | "syscall"
            | "sysenter"
            | "sysexit"
            | "sysret"
            | "test"
            | "ud2"
            | "verr"
            | "verw"
            | "wait"
            | "wbinvd"
            | "wrmsr"
            | "xadd"
            | "xchg"
            | "xlatb"
            | "xor"
            | "pxor"
            | "xorps"
            | "xorpd"
            | "movaps"
            | "movups"
            | "movapd"
            | "movupd"
            | "movd"
            | "movq"
            | "movdqa"
            | "movdqu"
            | "movss"
            | "addps"
            | "addpd"
            | "addss"
            | "addsd"
            | "subps"
            | "subpd"
            | "subss"
            | "subsd"
            | "mulps"
            | "mulpd"
            | "mulss"
            | "mulsd"
            | "divps"
            | "divpd"
            | "divss"
            | "divsd"
    ) || ["j", "cmov", "set"].iter().any(|prefix| {
        mnemonic.strip_prefix(prefix).is_some_and(|condition| {
            matches!(
                condition,
                "a" | "ae"
                    | "b"
                    | "be"
                    | "c"
                    | "e"
                    | "g"
                    | "ge"
                    | "l"
                    | "le"
                    | "na"
                    | "nae"
                    | "nb"
                    | "nbe"
                    | "nc"
                    | "ne"
                    | "ng"
                    | "nge"
                    | "nl"
                    | "nle"
                    | "no"
                    | "np"
                    | "ns"
                    | "nz"
                    | "o"
                    | "p"
                    | "pe"
                    | "po"
                    | "s"
                    | "z"
            )
        })
    })
}

fn validate_instruction(source: &str) -> Result<&str, String> {
    // Do not trim before rejecting newline/control characters: even a trailing
    // newline must not turn an assembler program into an accepted instruction.
    if source.len() > MAX_ASSEMBLY_SOURCE
        || !source
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b" \t_[](),+-*/$<> &|^~".contains(&byte))
    {
        return Err("Enter one instruction (at most 512 ASCII bytes). Newlines, labels, comments, quotes, macros and directives are unsupported.".into());
    }
    let source = source.trim();
    let mut words = source.split_ascii_whitespace();
    let mnemonic = loop {
        let word = words.next().ok_or("Enter an instruction to assemble.")?;
        let word = word.to_ascii_lowercase();
        if !matches!(
            word.as_str(),
            "lock" | "rep" | "repe" | "repz" | "repne" | "repnz"
        ) {
            break word;
        }
    };
    if !is_supported_mnemonic(&mnemonic) {
        return Err(format!(
            "Unsupported instruction mnemonic '{mnemonic}'. This helper accepts a single supported x86 instruction; assembler directives and macros are disabled."
        ));
    }
    Ok(source)
}

fn run_bounded(command: &mut Command, timeout: Duration) -> Result<(ExitStatus, String), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start the assembler ({error}). Install NASM at {NASM_PATH} to enable assembly."))?;
    let mut stderr = child.stderr.take().expect("stderr was piped");
    // Drain continuously so a full pipe cannot hang the child, retaining a fixed
    // diagnostic budget. The child is always killed/reaped before joining.
    let diagnostics = thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = [0; 1024];
        loop {
            match stderr.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    let remaining = MAX_DIAGNOSTIC_BYTES - kept.len();
                    kept.extend_from_slice(&chunk[..count.min(remaining)]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&kept).into_owned()
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(
                    "Assembly timed out. The assembler was stopped; no patch was applied.".into(),
                );
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "Could not wait for the assembler ({error}). No patch was applied."
                ));
            }
        }
    };
    let diagnostics = diagnostics.join().unwrap_or_default();
    Ok((status?, diagnostics))
}

/// Assemble one supported x86 instruction at its actual address. No shell is used,
/// no user code is executed, and source/output/process lifetime are bounded.
pub(crate) fn assemble_instruction(
    source: &str,
    bitness: u32,
    origin: u64,
) -> Result<Vec<u8>, String> {
    if !matches!(bitness, 16 | 32 | 64) {
        return Err("Choose x86 16-bit, 32-bit or 64-bit assembly.".into());
    }
    let instruction = validate_instruction(source)?;
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let input = directory.path().join("instruction.asm");
    let output = directory.path().join("instruction.bin");
    fs::write(
        &input,
        format!("[BITS {bitness}]\n[ORG {origin}]\n{instruction}\n"),
    )
    .map_err(|error| error.to_string())?;
    let mut command = Command::new(NASM_PATH);
    command
        // Ignore ambient assembler options/include paths and disable preprocessing.
        .env_remove("NASMENV")
        .env_remove("NASM_INCLUDE")
        .current_dir(directory.path())
        .args(["-a", "-f", "bin", "-Werror", "-o"])
        .arg(&output)
        .arg(&input);
    let (status, diagnostics) = run_bounded(&mut command, ASSEMBLY_TIMEOUT)?;
    if !status.success() {
        let diagnostic = diagnostics.trim();
        return Err(if diagnostic.is_empty() {
            format!("The assembler exited with {status}. No patch was applied.")
        } else {
            format!("Assembly failed: {diagnostic}")
        });
    }
    let file = File::open(output).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_INSTRUCTION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_INSTRUCTION_BYTES {
        return Err(
            "Assembly must produce one instruction of 1 to 15 bytes. No patch was applied.".into(),
        );
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_exact_binary_bytes_and_rejects_nonfiles_and_oversize() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("binary");
        let bytes = [0, 255, 128, b'\r', b'\n', 0x90, 0xc3];
        fs::write(&path, bytes).unwrap();
        assert_eq!(read_binary(&path).unwrap(), bytes);
        assert!(read_binary(directory.path()).is_err());
        let large = File::create(directory.path().join("large")).unwrap();
        large.set_len(MAX_BINARY_BYTES + 1).unwrap();
        assert!(
            read_binary(&directory.path().join("large"))
                .unwrap_err()
                .contains("64 MiB")
        );
    }

    #[test]
    fn changed_or_replaced_input_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("binary");
        fs::write(&path, b"old").unwrap();
        assert!(read_binary_impl(&path, || fs::write(&path, b"changed").unwrap()).is_err());
        fs::write(&path, b"old").unwrap();
        let replacement = directory.path().join("replacement");
        fs::write(&replacement, b"new").unwrap();
        assert!(read_binary_impl(&path, || fs::rename(&replacement, &path).unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_symlink_parents_are_rejected() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::write(&source, b"source").unwrap();
        let link = directory.path().join("link");
        symlink(&source, &link).unwrap();
        assert!(read_binary(&link).is_err());
        assert!(export_copy(&link, b"patch").is_err());
        let parent = directory.path().join("linked-parent");
        symlink(directory.path(), &parent).unwrap();
        assert!(read_binary(&parent.join("source")).is_err());
        assert!(export_copy(&parent.join("new"), b"patch").is_err());
        assert!(!directory.path().join("new").exists());
        assert_eq!(fs::read(&source).unwrap(), b"source");
    }

    #[test]
    fn exports_new_copy_and_never_overwrites_a_collision() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let output = directory.path().join("output");
        fs::write(&source, [0, 128, 255]).unwrap();
        export_copy(&output, &[0x90, 0xc3]).unwrap();
        assert_eq!(fs::read(&output).unwrap(), [0x90, 0xc3]);
        assert!(export_copy(&source, &[0x90]).is_err());
        assert_eq!(fs::read(&source).unwrap(), [0, 128, 255]);
        let race = directory.path().join("race");
        assert!(
            export_copy_impl(&race, b"patch", || fs::write(&race, b"other writer")
                .unwrap())
            .is_err()
        );
        assert_eq!(fs::read(&race).unwrap(), b"other writer");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 3);
        assert!(export_copy(&directory.path().join("missing").join("out"), b"patch").is_err());
        assert_eq!(fs::read(&source).unwrap(), [0, 128, 255]);
    }

    #[test]
    fn assembler_input_excludes_programs_macros_and_directives() {
        for source in [
            "",
            "db 0x90",
            "times 10 nop",
            "incbin file",
            "%include file",
            "label: nop",
            "nop\nnop",
            "nop\r",
            "nop ; comment",
            "[bits 64]",
            "bits 32",
            "org 0",
            "section text",
            "struc foo",
            "resb 100",
            "lock incbin file",
            "mov eax, %foo",
            "mov eax, 'A'",
            "mov eax, `value`",
            "nop\\",
            "macro nop",
            "nop\0",
        ] {
            assert!(
                validate_instruction(source).is_err(),
                "accepted: {source:?}"
            );
        }
        assert!(validate_instruction(&"nop ".repeat(129)).is_err());
        assert!(assemble_instruction("nop", 7, 0).is_err());
        assert!(validate_instruction("lock add dword [rax], 1").is_ok());
        assert!(validate_instruction("jmp 0x401010").is_ok());
    }

    #[test]
    fn actual_nasm_encodes_instructions_at_the_requested_origin() {
        if !Path::new(NASM_PATH).is_file() {
            eprintln!(
                "SKIP: NASM is unavailable at {NASM_PATH}; actual assembler acceptance remains unverified."
            );
            return;
        }
        assert_eq!(
            assemble_instruction("mov eax, 42", 64, 0x401000).unwrap(),
            [0xb8, 42, 0, 0, 0]
        );
        assert_eq!(
            assemble_instruction("jmp 0x401010", 64, 0x401000).unwrap(),
            [0xe9, 0x0b, 0, 0, 0]
        );
        assert_eq!(
            assemble_instruction("jmp short 0x401010", 64, 0x401000).unwrap(),
            [0xeb, 0x0e]
        );
        assert_eq!(
            assemble_instruction("mov ax, 42", 16, 0x7c00).unwrap(),
            [0xb8, 42, 0]
        );
        assert_eq!(assemble_instruction("nop", 32, 0).unwrap(), [0x90]);
        assert!(assemble_instruction("mov eax, definitely_not_a_register", 64, 0).is_err());
        assert!(assemble_instruction("mov al, 999999", 64, 0).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn child_process_timeout_is_enforced() {
        let started = Instant::now();
        let error = run_bounded(
            Command::new("/bin/sleep").arg("5"),
            Duration::from_millis(20),
        )
        .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
