//! Optional one-shot native worker. Call only on the background executor.
//! The configured worker is trusted executable code, not a sandboxed plugin.
use ale_systems_core::{Analysis, AnalyzedFunction, BinaryImage};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    ops::Range,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const PROTOCOL: &str = "ale-decompiler-v1";
const REVISION: &str = "c4273522017788fb67c30058ffd5bbdf291fcc40";
const MAX_RESPONSE: usize = 16 * 1024 * 1024;
const MAX_TEXT: usize = 1024 * 1024;
const MAX_TOKENS: usize = 100_000;

#[derive(Debug)]
pub(crate) struct Token {
    pub range: Range<usize>,
    pub offset: Option<usize>,
    pub kind: String,
}
#[derive(Debug)]
pub(crate) struct Decompiled {
    pub entry: u64,
    pub text: String,
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<String>,
}

fn function_name(function: &AnalyzedFunction) -> String {
    let name = &function.name;
    if name.len() <= 128
        && name.bytes().enumerate().all(|(i, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || i > 0 && byte.is_ascii_digit()
        })
        && !name.is_empty()
    {
        name.clone()
    } else {
        format!("sub_{:x}", function.address)
    }
}

fn request(
    image: &BinaryImage,
    analysis: &Analysis,
    function: &AnalyzedFunction,
) -> Result<Vec<u8>, String> {
    let regions = image.decompiler_regions()?;
    if image.address_to_offset(function.address) != Some(function.offset)
        || function.blocks.is_empty()
    {
        return Err("Choose a recovered function with an unambiguous mapped entry".into());
    }
    let mut names = BTreeSet::new();
    let functions: Vec<_> = analysis
        .functions
        .iter()
        .map(|function| {
            let mut name = function_name(function);
            if !names.insert(name.clone()) {
                name = format!("sub_{:x}", function.address);
                names.insert(name.clone());
            }
            json!({"address": format!("0x{:x}", function.address), "name": name})
        })
        .collect();
    let segments: Vec<_> = regions.iter().map(|region| {
        let mut hex = String::with_capacity(region.bytes.len() * 2);
        const DIGITS: &[u8] = b"0123456789abcdef";
        for byte in region.bytes { hex.push(DIGITS[(byte >> 4) as usize] as char); hex.push(DIGITS[(byte & 15) as usize] as char); }
        json!({"address": format!("0x{:x}", region.address), "bytes_hex": hex, "readonly": region.readonly})
    }).collect();
    serde_json::to_vec(
        &json!({"protocol": PROTOCOL, "architecture": "x86:LE:64:default:gcc",
        "entry": format!("0x{:x}", function.address), "name": function_name(function),
        "segments": segments, "functions": functions}),
    )
    .map_err(|error| error.to_string())
}

fn hex_address(value: &Value) -> Result<u64, String> {
    value
        .as_str()
        .and_then(|s| s.strip_prefix("0x"))
        .filter(|s| !s.is_empty() && s.len() <= 16 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|s| u64::from_str_radix(s, 16).ok())
        .ok_or_else(|| "Invalid native address".into())
}

fn response(
    bytes: &[u8],
    image: &BinaryImage,
    function: &AnalyzedFunction,
) -> Result<Decompiled, String> {
    if bytes.len() > MAX_RESPONSE {
        return Err("Native response exceeds limit".into());
    }
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("Invalid native response: {error}"))?;
    if value["protocol"] != PROTOCOL
        || value["backend"]["name"] != "ghidra-native"
        || value["backend"]["revision"] != REVISION
    {
        return Err("Incompatible native worker protocol or backend revision".into());
    }
    if value["ok"] != true {
        let error = value["error"]
            .as_str()
            .unwrap_or("Native worker rejected the request");
        return Err(error.chars().take(4096).collect());
    }
    let entry = hex_address(&value["entry"])?;
    if entry != function.address {
        return Err("Native response belongs to a different function".into());
    }
    let text = value["pseudocode"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= MAX_TEXT)
        .ok_or("Native pseudocode is empty or exceeds limit")?
        .to_owned();
    let values = value["tokens"]
        .as_array()
        .filter(|tokens| tokens.len() <= MAX_TOKENS)
        .ok_or("Invalid or excessive native tokens")?;
    let known: BTreeSet<_> = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .map(|instruction| instruction.address)
        .collect();
    let mut tokens = Vec::with_capacity(values.len());
    let mut previous = 0;
    for token in values {
        let start = token["start"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("Invalid token start")?;
        let end = token["end"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("Invalid token end")?;
        if start < previous
            || start >= end
            || text.get(start..end)
                != Some(
                    token["text"]
                        .as_str()
                        .ok_or("Native token text is missing")?,
                )
        {
            return Err("Native token ranges overlap or do not match UTF-8 pseudocode".into());
        }
        let offset = match token.get("address") {
            Some(Value::Null) => None,
            Some(address) => {
                let address = hex_address(address)?;
                if !known.contains(&address) {
                    return Err(
                        "Native token address is not a recovered instruction in this function"
                            .into(),
                    );
                }
                Some(
                    image
                        .address_to_offset(address)
                        .ok_or("Native token address has no unique file mapping")?,
                )
            }
            None => return Err("Native token lacks address field".into()),
        };
        let kind = token["kind"]
            .as_str()
            .filter(|s| s.len() <= 64)
            .ok_or("Invalid token kind")?
            .to_owned();
        tokens.push(Token {
            range: start..end,
            offset,
            kind,
        });
        previous = end;
    }
    let diagnostics = value["diagnostics"]
        .as_array()
        .filter(|v| v.len() <= 64)
        .ok_or("Invalid native diagnostics")?
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| s.len() <= 4096)
                .map(str::to_owned)
                .ok_or_else(|| "Invalid native diagnostic".to_owned())
        })
        .collect::<Result<_, _>>()?;
    Ok(Decompiled {
        entry,
        text,
        tokens,
        diagnostics,
    })
}

pub(crate) fn decompile(
    worker: &Path,
    image: &BinaryImage,
    analysis: &Analysis,
    function: &AnalyzedFunction,
    cancel: &AtomicBool,
) -> Result<Decompiled, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Decompilation cancelled".into());
    }
    if !worker.is_absolute() {
        return Err("ALE_DECOMPILER must name an absolute native worker path".into());
    }
    let input = request(image, analysis, function)?;
    let output = run_worker(
        worker,
        &input,
        cancel,
        Duration::from_secs(20),
        MAX_RESPONSE,
    )?;
    response(&output, image, function)
}

#[cfg(not(target_os = "linux"))]
fn run_worker(
    _: &Path,
    _: &[u8],
    _: &AtomicBool,
    _: Duration,
    _: usize,
) -> Result<Vec<u8>, String> {
    Err("Native decompiler process isolation currently requires Linux".into())
}

#[cfg(target_os = "linux")]
fn run_worker(
    path: &Path,
    input: &[u8],
    cancel: &AtomicBool,
    timeout: Duration,
    output_limit: usize,
) -> Result<Vec<u8>, String> {
    use std::{
        io::{Read, Write},
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Child, Command, Stdio},
        time::Instant,
    };
    struct ChildGroup(Child);
    impl Drop for ChildGroup {
        fn drop(&mut self) {
            // Each worker has its own process group; descendants inherit it.
            // The configured worker is trusted not to escape that group.
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }
    fn nonblocking(fd: i32) -> Result<(), String> {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        Ok(())
    }
    fn drain(stream: &mut impl Read, buffer: &mut Vec<u8>, limit: usize) -> Result<bool, String> {
        let mut chunk = [0; 8192];
        // Bound each pass so a streaming worker cannot starve cancellation.
        for _ in 0..16 {
            match stream.read(&mut chunk) {
                Ok(0) => return Ok(true),
                Ok(n) => {
                    if buffer.len() + n > limit {
                        return Err("Native worker output exceeded limit".into());
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(false)
    }
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut command = Command::new(path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(directory.path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("TMPDIR", directory.path())
        .process_group(0);
    // Only async-signal-safe libc calls execute between fork and exec.
    unsafe {
        command.pre_exec(|| {
            for (resource, limit) in [
                (libc::RLIMIT_CPU, 18),
                (libc::RLIMIT_AS, 2 * 1024 * 1024 * 1024),
                (libc::RLIMIT_FSIZE, 16 * 1024 * 1024),
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
    let mut child = ChildGroup(
        command
            .spawn()
            .map_err(|e| format!("Cannot start native worker: {e}"))?,
    );
    let mut stdin = child.0.stdin.take();
    let mut stdout = child.0.stdout.take().ok_or("Native stdout unavailable")?;
    let mut stderr = child.0.stderr.take().ok_or("Native stderr unavailable")?;
    nonblocking(
        stdin
            .as_ref()
            .ok_or("Native stdin unavailable")?
            .as_raw_fd(),
    )?;
    nonblocking(stdout.as_raw_fd())?;
    nonblocking(stderr.as_raw_fd())?;
    let started = Instant::now();
    let mut sent = 0;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Decompilation cancelled".into());
        }
        if started.elapsed() >= timeout {
            return Err("Native decompiler timed out".into());
        }
        if let Some(pipe) = &mut stdin {
            if sent == input.len() {
                stdin = None;
            } else {
                match pipe.write(&input[sent..input.len().min(sent + 131_072)]) {
                    Ok(n) => sent += n,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => stdin = None,
                    Err(error) => return Err(error.to_string()),
                }
            }
        }
        let out_done = drain(&mut stdout, &mut output, output_limit)?;
        let err_done = drain(&mut stderr, &mut errors, 64 * 1024)?;
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())?
            && out_done
            && err_done
        {
            if !status.success() {
                // Error JSON remains useful, but a failed process can never publish success.
                let detail = serde_json::from_slice::<Value>(&output)
                    .ok()
                    .and_then(|v| v["error"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| String::from_utf8_lossy(&errors).into_owned());
                return Err(format!(
                    "Native worker exited {status}: {}",
                    detail.chars().take(4096).collect::<String>()
                ));
            }
            if sent != input.len() {
                return Err("Native worker exited before reading the snapshot".into());
            }
            return Ok(output);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ale_systems_core::{AnalysisLimits, Architecture};

    pub(crate) fn fixture() -> (BinaryImage, Analysis) {
        let mut bytes = vec![0; 0x300];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x401000u64.to_le_bytes());
        bytes[40..48].copy_from_slice(&0x200u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes[58..60].copy_from_slice(&64u16.to_le_bytes());
        bytes[60..62].copy_from_slice(&3u16.to_le_bytes());
        bytes[62..64].copy_from_slice(&2u16.to_le_bytes());
        bytes[0x100..0x104].copy_from_slice(&[0x8d, 0x47, 0x01, 0xc3]);
        let names = b"\0.text\0.shstrtab\0";
        bytes[0x180..0x180 + names.len()].copy_from_slice(names);
        for (at, name, kind, flags, address, offset, size) in [
            (0x240, 1u32, 1u32, 6u64, 0x401000u64, 0x100u64, 4u64),
            (0x280, 7, 3, 0, 0, 0x180, names.len() as u64),
        ] {
            bytes[at..at + 4].copy_from_slice(&name.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&kind.to_le_bytes());
            for (delta, value) in [(8, flags), (16, address), (24, offset), (32, size), (48, 1)] {
                bytes[at + delta..at + delta + 8].copy_from_slice(&value.to_le_bytes());
            }
        }
        let image = BinaryImage::parse(bytes).unwrap();
        let analysis = image
            .analyze(
                &Architecture::X86_64,
                AnalysisLimits::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
        (image, analysis)
    }

    fn valid_response() -> Value {
        json!({"protocol": PROTOCOL, "backend": {"name": "ghidra-native", "revision": REVISION}, "ok": true,
            "entry": "0x401000", "pseudocode": "é + 1", "diagnostics": [],
            "tokens": [{"start": 0, "end": 2, "text": "é", "kind": "variable", "address": "0x401000"}]})
    }

    #[test]
    fn strict_response_checks_unicode_revision_entry_ranges_and_addresses() {
        let (image, analysis) = fixture();
        let function = &analysis.functions[0];
        let valid = valid_response();
        let parsed = response(&serde_json::to_vec(&valid).unwrap(), &image, function).unwrap();
        assert_eq!(parsed.tokens[0].offset, Some(0x100));
        for invalid in [
            {
                let mut v = valid.clone();
                v["tokens"][0]["end"] = json!(1);
                v
            },
            {
                let mut v = valid.clone();
                v["tokens"][0]["address"] = json!("0x401001");
                v
            },
            {
                let mut v = valid.clone();
                v["entry"] = json!("0x401003");
                v
            },
            {
                let mut v = valid.clone();
                v["backend"]["revision"] = json!("wrong");
                v
            },
            {
                let mut v = valid.clone();
                v["tokens"] = json!([v["tokens"][0], v["tokens"][0]]);
                v
            },
            {
                let mut v = valid.clone();
                v["tokens"][0]["text"] = json!("wrong");
                v
            },
        ] {
            assert!(response(&serde_json::to_vec(&invalid).unwrap(), &image, function).is_err());
        }
        let encoded: Value =
            serde_json::from_slice(&request(&image, &analysis, function).unwrap()).unwrap();
        assert_eq!(encoded["segments"][0]["bytes_hex"], "8d4701c3");
        assert_eq!(encoded["segments"][0]["readonly"], true);
    }

    #[test]
    fn configured_native_worker_decompiles_captured_fixture() {
        let Some(path) = std::env::var_os("ALE_DECOMPILER_TEST") else {
            eprintln!(
                "SKIP: set ALE_DECOMPILER_TEST to the pinned native worker for actual backend acceptance"
            );
            return;
        };
        let (image, analysis) = fixture();
        let output = decompile(
            Path::new(&path),
            &image,
            &analysis,
            &analysis.functions[0],
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(output.text.contains("+ 1"), "{}", output.text);
        assert!(
            output
                .tokens
                .iter()
                .any(|token| token.offset == Some(0x100))
        );
        assert!(
            output
                .tokens
                .iter()
                .any(|token| token.offset == Some(0x103))
        );
    }

    #[cfg(target_os = "linux")]
    fn script(directory: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.path().join("worker");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_limits_timeout_cancellation_and_exit_status_are_enforced() {
        let directory = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let path = script(&directory, "cat >/dev/null; printf response");
        assert_eq!(
            run_worker(&path, b"snapshot", &cancel, Duration::from_secs(2), 100).unwrap(),
            b"response"
        );
        script(&directory, "yes x");
        assert!(
            run_worker(&path, b"", &cancel, Duration::from_secs(2), 100)
                .unwrap_err()
                .contains("limit")
        );
        script(&directory, "sleep 60 & wait");
        let started = std::time::Instant::now();
        assert!(
            run_worker(&path, b"", &cancel, Duration::from_millis(100), 100)
                .unwrap_err()
                .contains("timed out")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                cancel.store(true, Ordering::Relaxed);
            });
            assert!(
                run_worker(&path, b"", &cancel, Duration::from_secs(2), 100)
                    .unwrap_err()
                    .contains("cancelled")
            );
        });
        cancel.store(false, Ordering::Relaxed);
        script(&directory, "printf success; exit 1");
        assert!(
            run_worker(&path, b"", &cancel, Duration::from_secs(2), 100)
                .unwrap_err()
                .contains("exited")
        );
        script(&directory, "sleep 60 & exit 0");
        assert!(
            run_worker(&path, b"", &cancel, Duration::from_millis(100), 100)
                .unwrap_err()
                .contains("timed out")
        );
    }
}
