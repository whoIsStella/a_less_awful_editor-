//! Independent acceptance against the host compiler and GNU binutils. The
//! controlled fixture is compiled into a temporary directory and never executed.

use ale_systems_core::{Architecture, BinaryFormat, BinaryImage};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

struct FixtureDirectory(PathBuf);

impl FixtureDirectory {
    fn new() -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ale-toolchain-oracle-{}-{timestamp}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .expect("create private fixture directory");
        Self(path)
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn optional_tool_available(tool: &str) -> bool {
    match Command::new(tool)
        .arg("--version")
        .env("LC_ALL", "C")
        .output()
    {
        Ok(output) if output.status.success() => true,
        Ok(output) => panic!(
            "{tool} --version failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "SKIP: optional tool '{tool}' is unavailable; ELF/DWARF toolchain-oracle acceptance remains unverified."
            );
            false
        }
        Err(error) => panic!("cannot start {tool}: {error}"),
    }
}

fn successful_output(command: &mut Command) -> Output {
    let output = command
        .env("LC_ALL", "C")
        .output()
        .expect("run fixture tool");
    assert!(
        output.status.success(),
        "{command:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn output_text(command: &mut Command) -> String {
    String::from_utf8(successful_output(command).stdout).expect("tool output is UTF-8")
}

fn hex(value: &str) -> u64 {
    u64::from_str_radix(value.trim_start_matches("0x"), 16).expect("oracle hexadecimal number")
}

struct OracleInstruction {
    address: u64,
    bytes: Vec<u8>,
    text: String,
}

fn objdump_instructions(text: &str) -> Vec<OracleInstruction> {
    text.lines()
        .filter_map(|line| {
            let (address, body) = line.split_once(':')?;
            let address = address.trim();
            if address.is_empty() || !address.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            let mut tokens = body.split_ascii_whitespace().peekable();
            let mut bytes = Vec::new();
            while tokens.peek().is_some_and(|token| {
                token.len() == 2 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
            }) {
                bytes.push(u8::from_str_radix(tokens.next().unwrap(), 16).unwrap());
            }
            if bytes.is_empty() {
                return None;
            }
            let text = tokens.collect::<Vec<_>>().join(" ");
            assert!(
                !text.is_empty(),
                "objdump wrapped an instruction despite --insn-width=15"
            );
            Some(OracleInstruction {
                address: hex(address),
                bytes,
                text,
            })
        })
        .collect()
}

#[test]
fn compiled_elf_matches_readelf_objdump_and_addr2line() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        eprintln!(
            "SKIP: this controlled ELF/x86-64 toolchain fixture requires Linux x86-64; other formats have separate unit coverage."
        );
        return;
    }
    for tool in ["cc", "readelf", "objdump", "addr2line"] {
        if !optional_tool_available(tool) {
            return;
        }
    }
    let directory = FixtureDirectory::new();
    let source = directory.0.join("fixture.c");
    let binary = directory.0.join("fixture.elf");
    fs::write(
        &source,
        concat!(
            "volatile int sample_counter = 3;\n",
            "__attribute__((noinline)) int inspected_fixture(int input) {\n",
            "    int adjusted = input + sample_counter;\n",
            "    if (adjusted > 10) {\n",
            "        adjusted -= 2;\n",
            "    } else {\n",
            "        adjusted += 4;\n",
            "    }\n",
            "    return adjusted ^ 0x55;\n",
            "}\n",
            "int main(void) {\n",
            "    return inspected_fixture(7);\n",
            "}\n",
        ),
    )
    .unwrap();
    successful_output(
        Command::new("cc")
            .current_dir(&directory.0)
            .args([
                "-g",
                "-O0",
                "-fno-inline",
                "-fno-omit-frame-pointer",
                "-fno-pie",
                "-no-pie",
            ])
            .arg(&source)
            .arg("-o")
            .arg(&binary),
    );
    // Inspect bytes only. No command ever launches fixture.elf.
    let image = BinaryImage::parse(fs::read(&binary).unwrap()).unwrap();
    assert_eq!(image.format, BinaryFormat::Elf);
    assert_eq!(image.architecture, Architecture::X86_64);
    assert!(
        image.warnings.is_empty(),
        "unexpected analysis warnings: {:?}",
        image.warnings
    );

    let symbols = output_text(Command::new("readelf").args(["-W", "-s"]).arg(&binary));
    let symbol_row: Vec<_> = symbols
        .lines()
        .map(|line| line.split_ascii_whitespace().collect::<Vec<_>>())
        .find(|columns| columns.len() >= 8 && columns[7] == "inspected_fixture")
        .expect("readelf fixture symbol");
    let symbol_address = hex(symbol_row[1]);
    let symbol_size = symbol_row[2].parse::<u64>().unwrap();
    assert!(symbol_size > 0);
    assert_eq!(symbol_row[3], "FUNC");
    let symbol = image
        .symbols
        .iter()
        .find(|symbol| symbol.name == "inspected_fixture")
        .unwrap();
    assert_eq!(symbol.address, symbol_address);
    assert_eq!(symbol.size, symbol_size);

    let sections = output_text(Command::new("readelf").args(["-W", "-S"]).arg(&binary));
    let section_row: Vec<_> = sections
        .lines()
        .map(|line| line.split_ascii_whitespace().collect::<Vec<_>>())
        .find(|columns| columns.contains(&".text"))
        .expect("readelf text section");
    let name_column = section_row
        .iter()
        .position(|column| *column == ".text")
        .unwrap();
    let text_address = hex(section_row[name_column + 2]);
    let text_offset = hex(section_row[name_column + 3]) as usize;
    let text_size = hex(section_row[name_column + 4]);
    let section = image
        .sections
        .iter()
        .find(|section| section.name == ".text")
        .unwrap();
    assert_eq!(section.address, text_address);
    assert_eq!(section.file_offset, text_offset);
    assert_eq!(section.size, text_size);
    assert_eq!(section.file_size as u64, text_size);
    assert!(section.executable);
    let symbol_offset = text_offset + usize::try_from(symbol_address - text_address).unwrap();
    assert_eq!(symbol.file_offset, Some(symbol_offset));
    assert_eq!(image.address_to_offset(symbol_address), Some(symbol_offset));
    assert_eq!(image.offset_to_address(symbol_offset), Some(symbol_address));

    let disassembly = output_text(
        Command::new("objdump")
            .args([
                "-d",
                "-Mintel",
                "--insn-width=15",
                "--disassemble=inspected_fixture",
            ])
            .arg(&binary),
    );
    let expected = objdump_instructions(&disassembly);
    assert!(
        expected.len() > 8,
        "fixture should contain meaningful instructions and branches"
    );
    let actual = image
        .disassemble(symbol_offset, symbol_size as usize, 256)
        .unwrap();
    assert_eq!(actual.len(), expected.len());
    let mut branch_count = 0;
    for (actual, expected) in actual.iter().zip(&expected) {
        assert!(actual.valid);
        assert_eq!(actual.address, expected.address);
        assert_eq!(
            actual.bytes, expected.bytes,
            "instruction at {:#x}",
            expected.address
        );
        assert_eq!(
            actual.text.split_ascii_whitespace().next(),
            expected.text.split_ascii_whitespace().next()
        );
        assert_eq!(
            image.offset_to_address(actual.offset),
            Some(expected.address)
        );
        assert_eq!(
            &image.bytes()[actual.offset..actual.offset + actual.bytes.len()],
            expected.bytes
        );
        if let Some(target) = actual.branch_target {
            branch_count += 1;
            assert_eq!(
                target,
                hex(expected.text.split_ascii_whitespace().nth(1).unwrap())
            );
        }
    }
    assert!(
        branch_count >= 2,
        "conditional and unconditional branch destinations must be checked"
    );

    // Recursive recovery must independently agree with GNU instruction boundaries
    // and direct branch destinations for this fully reachable controlled function.
    let analysis = image
        .analyze(
            &Architecture::X86_64,
            ale_systems_core::AnalysisLimits::default(),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
    let function = analysis
        .functions
        .iter()
        .find(|function| function.address == symbol_address)
        .unwrap();
    let recovered: std::collections::BTreeMap<_, _> = function
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter())
        .map(|instruction| (instruction.address, &instruction.bytes))
        .collect();
    assert_eq!(recovered.len(), expected.len());
    for instruction in &expected {
        assert_eq!(
            recovered.get(&instruction.address).copied(),
            Some(&instruction.bytes)
        );
        if instruction.text.starts_with('j') {
            let target = hex(instruction.text.split_ascii_whitespace().nth(1).unwrap());
            assert!(
                analysis
                    .references
                    .iter()
                    .any(|reference| reference.from_address == instruction.address
                        && reference.target_address == Some(target)
                        && reference.resolution == ale_systems_core::EdgeResolution::Resolved)
            );
        }
    }

    // Check both instruction starts and interior bytes, so source lookup must
    // use line-table intervals instead of matching only exact DWARF row addresses.
    let addresses: Vec<_> = actual
        .iter()
        .flat_map(|instruction| {
            [
                instruction.address,
                instruction.address + instruction.bytes.len() as u64 - 1,
            ]
        })
        .collect();
    let locations = output_text(
        Command::new("addr2line")
            .args(["-f", "-e"])
            .arg(&binary)
            .args(addresses.iter().map(|address| format!("0x{address:x}"))),
    );
    let oracle_lines: Vec<_> = locations.lines().collect();
    assert_eq!(oracle_lines.len(), addresses.len() * 2);
    let mut observed_lines = std::collections::BTreeSet::new();
    for (address, oracle) in addresses.iter().zip(oracle_lines.chunks_exact(2)) {
        assert_eq!(oracle[0], "inspected_fixture");
        let (path, line) = oracle[1].rsplit_once(':').unwrap();
        let line = line
            .split_ascii_whitespace()
            .next()
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assert!(line > 0);
        let offset = image.address_to_offset(*address).unwrap();
        let location = image.source_at_offset(offset).unwrap_or_else(|| {
            panic!(
                "missing source mapping at {address:#x}; warnings: {:?}",
                image.warnings
            )
        });
        assert_eq!(PathBuf::from(&location.path), PathBuf::from(path));
        assert_eq!(PathBuf::from(path), source);
        assert_eq!(location.line, line, "source line at {address:#x}");
        assert!(*address >= location.address && *address < location.end_address);
        assert_eq!(
            location.file_offset,
            image.address_to_offset(location.address)
        );
        observed_lines.insert(line);
    }
    assert!(
        observed_lines.len() >= 5,
        "fixture should exercise multiple source-line intervals"
    );
}
