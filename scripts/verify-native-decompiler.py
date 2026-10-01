#!/usr/bin/env python3
"""Opt-in native Ghidra experiment; no installation or IDE integration.

Requires Python 3.9+, Linux x86-64, a C/C++ compiler, make, binutils, and zlib
development files. Sources and generated files stay in --work-dir. Cached native
binaries are reused; this is a fixture verification, not a reproducible-build
attestation. See docs/systems-capabilities.md for the acceptance boundary.
"""

import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET


REVISION = "c4273522017788fb67c30058ffd5bbdf291fcc40"
# SHA-256 of sorted `path + NUL + Git blob SHA-1 + newline` for the two subtrees.
MANIFEST_SHA256 = "560f78ffd679b23d2200bf714b174b0937e7599984f8e82f2edaadfe0299309e"
REPOSITORY = "https://github.com/NationalSecurityAgency/ghidra"
RAW = f"https://raw.githubusercontent.com/NationalSecurityAgency/ghidra/{REVISION}"
CPP = "Ghidra/Features/Decompiler/src/decompile/cpp"
LANGUAGES = "Ghidra/Processors/x86/data/languages"
EXCLUDED = "bfd_arch loadimage_bfd analyzesigs codedata"
MAX_DOWNLOAD = 16 * 1024 * 1024
DOWNLOAD_WORKER = """
import sys, urllib.request
try:
    request = urllib.request.Request(sys.argv[1], headers={"User-Agent": "ale-native-experiment"})
    with urllib.request.urlopen(request, timeout=20) as response:
        data = response.read(int(sys.argv[2]) + 1)
    if len(data) > int(sys.argv[2]):
        raise ValueError("download exceeds response size limit")
    sys.stdout.buffer.write(data)
except Exception as error:
    print(str(error), file=sys.stderr)
    sys.exit(1)
"""


class ExperimentError(Exception):
    pass


def download(url):
    """Two attempts with a 35s wall-clock bound, including a slow response body."""
    last = None
    for attempt in range(2):
        try:
            # A separate process makes the total deadline effective even if a
            # server keeps a socket alive by sending an arbitrarily slow body.
            result = subprocess.run(
                [sys.executable, "-c", DOWNLOAD_WORKER, url, str(MAX_DOWNLOAD)],
                capture_output=True, timeout=35, check=False,
            )
            if result.returncode == 0:
                return result.stdout
            last = result.stderr.decode(errors="replace")[-1000:]
        except (OSError, subprocess.TimeoutExpired) as error:
            last = error
        if attempt == 0:
            time.sleep(0.5)
    raise ExperimentError(f"Cannot download {url} after two attempts: {last}")


def git_blob_hash(data):
    return hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()


def write_atomic(path, data):
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix=path.name + ".", delete=False) as output:
        temporary = Path(output.name)
        output.write(data)
    try:
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def checked_command(root, name, argv, *, cwd=None, timeout=60, env=None):
    log = root / (name + ".log")
    with log.open("w") as output:
        process = subprocess.Popen(
            [str(arg) for arg in argv], cwd=cwd or root, env=env,
            stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            returncode = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            raise ExperimentError(f"{name} exceeded {timeout}s; process group stopped; see {log}")
    if returncode:
        detail = log.read_text(errors="replace")[-3000:]
        raise ExperimentError(f"{name} exited {returncode}; see {log}\n{detail}")
    return log.read_text(errors="replace")


def prepare_sources(root):
    manifest = root / "files.json"
    if not manifest.exists():
        tree = json.loads(download(f"https://api.github.com/repos/NationalSecurityAgency/ghidra/git/trees/{REVISION}?recursive=1"))
        if tree.get("truncated"):
            raise ExperimentError("GitHub returned a truncated source manifest")
        entries = [entry for entry in tree["tree"] if entry["type"] == "blob"
                   and entry["path"].startswith((CPP + "/", LANGUAGES + "/"))]
        write_atomic(manifest, json.dumps(entries, indent=2).encode())
    entries = json.loads(manifest.read_text())
    if not entries:
        raise ExperimentError("Empty pinned-source manifest")
    canonical = "".join(entry["path"] + "\0" + entry["sha"] + "\n"
                        for entry in sorted(entries, key=lambda entry: entry["path"]))
    if hashlib.sha256(canonical.encode()).hexdigest() != MANIFEST_SHA256:
        raise ExperimentError("Source manifest does not match the pinned official source set")
    expected = {}
    for entry in entries:
        name = entry["path"]
        if not name.startswith((CPP + "/", LANGUAGES + "/")) or ".." in Path(name).parts:
            raise ExperimentError(f"Unsafe source manifest path: {name}")
        expected[name] = entry["sha"]
    makefile_name = CPP + "/Makefile"
    if makefile_name not in expected:
        raise ExperimentError("Source manifest omits native Makefile")

    def ensure_source(name):
        path = root / name
        if root not in path.resolve().parents:
            raise ExperimentError(f"Cached source path escapes the work directory: {path}")
        path.parent.mkdir(parents=True, exist_ok=True)
        # The experiment's Makefile is deliberately changed after verification.
        original = path.with_name("Makefile.upstream") if name == makefile_name else path
        if original.exists() and git_blob_hash(original.read_bytes()) == expected[name]:
            return
        data = download(RAW + "/" + name)
        if git_blob_hash(data) != expected[name]:
            raise ExperimentError(f"Git blob hash mismatch: {name}")
        write_atomic(original, data)
        if name == makefile_name and not path.exists():
            write_atomic(path, data)

    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(ensure_source, expected))
    license_path = root / "UPSTREAM-LICENSE"
    if not license_path.exists():
        write_atomic(license_path, download(RAW + "/LICENSE"))
    return len(entries)


def run_experiment(args):
    if sys.version_info < (3, 9):
        raise ExperimentError("Python 3.9 or newer is required")
    if sys.platform != "linux" or platform.machine() != "x86_64":
        raise ExperimentError("This bounded experiment currently supports Linux x86-64 hosts only")
    root = args.work_dir.expanduser().resolve()
    repository_root = Path(__file__).resolve().parents[1]
    if root == repository_root or repository_root in root.parents:
        raise ExperimentError("Choose a work directory outside the repository; generated sources/binaries must stay out of it")
    marker = root / "revision"
    if root.exists() and any(root.iterdir()) and not marker.exists():
        raise ExperimentError("Nonempty work directory has no experiment revision marker; choose an empty directory")
    if root.exists() and any(path.is_symlink() for path in root.rglob("*")):
        raise ExperimentError("Work directory contains symbolic links; choose an ordinary private experiment directory")
    if marker.exists() and marker.read_text().strip() != REVISION:
        raise ExperimentError("Work directory belongs to a different source revision; choose another directory")
    root.mkdir(parents=True, exist_ok=True)
    marker.write_text(REVISION + "\n")
    binaries = {}
    for tool in ("cc", "c++", "g++", "make", "objcopy", "objdump"):
        located = shutil.which(tool)
        if not located:
            raise ExperimentError(f"Missing required tool: {tool}. Nothing will be installed automatically")
        binaries[tool] = located
    (root / "zlib-probe.cc").write_text("#include <zlib.h>\nint main() { return zlibVersion() ? 0 : 1; }\n")
    try:
        checked_command(root, "zlib-probe", [binaries["c++"], root / "zlib-probe.cc", "-lz", "-o", root / "zlib-probe"])
    except ExperimentError as error:
        raise ExperimentError(f"C++/zlib compile-link prerequisite failed. No dependencies were installed.\n{error}") from error

    print(f"Pinned upstream: {REVISION}\nWork directory: {root}", flush=True)
    count = prepare_sources(root)
    cpp = root / CPP
    languages = root / LANGUAGES
    upstream = (cpp / "Makefile.upstream").read_text()
    original = "$(SLACOMP) $(SPECIAL),$(ALL_NAMES))"
    replacement = f"$(SLACOMP) $(SPECIAL) {EXCLUDED},$(ALL_NAMES))"
    if upstream.count(original) != 1:
        raise ExperimentError("Pinned Makefile no longer matches the explicit BFD-module exclusion")
    (cpp / "Makefile").write_text(upstream.replace(original, replacement))
    reused = all((cpp / executable).is_file() for executable in ("decomp_opt", "sleigh_opt"))
    if not reused:
        # Upstream commits generated parsers; prevent fetch timestamps invoking flex/bison.
        for name in ("grammar.cc", "xml.cc", "pcodeparse.cc", "slghparse.cc", "slghparse.hh", "slghscan.cc", "ruleparse.cc", "ruleparse.hh"):
            path = cpp / name
            if path.exists():
                path.touch()
        (cpp / "sla_opt").mkdir(exist_ok=True)
        print("Building native decompiler and SLEIGH compiler...", flush=True)
        checked_command(root, "native-build", [binaries["make"], f"-j{args.jobs}", "decomp_opt", "sleigh_opt", "OPT_CXXFLAGS=-O0", "BFDLIB="], cwd=cpp, timeout=args.build_timeout)
    else:
        print("Reusing existing native binaries; no decompiler rebuild.", flush=True)
    native_env = dict(os.environ)
    native_env.pop("JAVA_HOME", None)
    native_env["PATH"] = "/nonexistent"
    if not (languages / "x86-64.sla").exists():
        checked_command(root, "native-sleigh", [cpp / "sleigh_opt", "x86-64.slaspec", "x86-64.sla"], cwd=languages, timeout=60, env=native_env)

    (root / "add_one.c").write_text("unsigned int add_one(unsigned int value) { return value + 1u; }\n")
    checked_command(root, "fixture-compile", [binaries["cc"], "-O1", "-fno-asynchronous-unwind-tables", "-fno-stack-protector", "-c", root / "add_one.c", "-o", root / "add_one.o"])
    checked_command(root, "fixture-extract", [binaries["objcopy"], "-O", "binary", "--only-section=.text", root / "add_one.o", root / "add_one.bin"])
    disassembly = checked_command(root, "independent-disassembly", [binaries["objdump"], "-d", root / "add_one.o"])
    fixture = (root / "add_one.bin").read_bytes()
    if fixture.hex() != "8d4701c3" or "lea" not in disassembly or "ret" not in disassembly:
        raise ExperimentError("Compiler produced a different fixture; expected 8d4701c3 (LEA; RET). Inspect independent-disassembly.log")
    image = ET.Element("binaryimage", arch="x86:LE:64:default:gcc")
    ET.SubElement(image, "bytechunk", space="ram", offset="0x1000", readonly="true").text = fixture.hex()
    ET.SubElement(image, "symbol", space="ram", offset="0x1000", name="add_one")
    ET.ElementTree(image).write(root / "image.xml", encoding="utf-8")
    # Use a relative image path: the native console parser does not quote spaces.
    (root / "commands.txt").write_text("load file image.xml\nload function add_one\ndecompile\nprint C\nprint C xml\nprint raw\nquit\n")
    output = checked_command(root, "native-decompile", [cpp / "decomp_opt", "-s", languages, "-i", root / "commands.txt"], timeout=20, env=native_env)
    begin, end = output.find("<function>"), output.find("</function>")
    if begin < 0 or end < begin or "return param_1 + 1;" not in output:
        raise ExperimentError("Decompiler did not return expected pseudocode and XML; see native-decompile.log")
    document = ET.fromstring(output[begin:end + len("</function>")])
    locations = {int(op, 16): int(address, 16) for address, op in re.findall(r"^(0x[0-9a-f]+):([0-9a-f]+):", output, re.M)}
    mapping = {}
    for token, address in (("+", 0x1000), ("return", 0x1003)):
        nodes = [node for node in document.iter("op") if node.text == token]
        if len(nodes) != 1 or locations.get(int(nodes[0].attrib.get("opref", "-1"), 0)) != address:
            raise ExperimentError(f"Pseudocode token {token!r} did not map to expected instruction {address:#x}")
        mapping[token] = hex(address)
    report = {
        "source_repository": REPOSITORY, "source_revision": REVISION,
        "source_file_count": count, "source_license": str(root / "UPSTREAM-LICENSE"),
        "source_manifest_sha256": MANIFEST_SHA256,
        "source_license_sha256": hashlib.sha256((root / "UPSTREAM-LICENSE").read_bytes()).hexdigest(),
        "excluded_bfd_console_modules": EXCLUDED.split(), "reused_native_binaries": reused,
        "native_binary_sha256": hashlib.sha256((cpp / "decomp_opt").read_bytes()).hexdigest(),
        "fixture_sha256": hashlib.sha256(fixture).hexdigest(), "token_instruction_mapping": mapping,
        "native_runtime_path": native_env["PATH"], "java_home": None,
        "scope": "One x86-64 fixture, native pseudocode and two operation mappings; not IDE integration, exact type recovery, or broad Ghidra parity",
    }
    (root / "native-experiment-result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"PASS: + -> 0x1000; return -> 0x1003\nEvidence: {root / 'native-experiment-result.json'}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--work-dir", required=True, type=Path, help="Explicit empty or previously pinned experiment directory outside the repository")
    parser.add_argument("--jobs", type=int, default=3, choices=range(1, 9), metavar="1..8", help="Native compiler jobs (default: 3)")
    parser.add_argument("--build-timeout", type=int, default=900, metavar="SECONDS", help="Maximum native build time before stopping the process group (default: 900)")
    args = parser.parse_args()
    if args.build_timeout < 1 or args.build_timeout > 3600:
        parser.error("--build-timeout must be between 1 and 3600 seconds")
    try:
        run_experiment(args)
    except (ExperimentError, OSError, ValueError, KeyError, TypeError, ET.ParseError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
