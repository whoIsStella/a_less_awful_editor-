#!/usr/bin/env python3
"""Controlled native worker acceptance. Compiles fixtures but never executes them."""
import argparse
import copy
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

PROTOCOL = "ale-decompiler-v1"
ARCHITECTURE = "x86:LE:64:default:gcc"


def run(worker, request, success=True):
    result = subprocess.run([str(worker)], input=json.dumps(request), text=True, capture_output=True, timeout=25)
    response = json.loads(result.stdout)
    assert response["protocol"] == PROTOCOL
    assert response["ok"] == success, response
    assert (result.returncode == 0) == success, result
    if success:
        text = response["pseudocode"].encode("utf-8")
        previous = 0
        for token in response["tokens"]:
            assert previous <= token["start"] < token["end"] <= len(text)
            assert text[token["start"]:token["end"]].decode() == token["text"]
            previous = token["end"]
    return response


def checked(argv):
    return subprocess.run(list(map(str, argv)), check=True, capture_output=True, text=True, timeout=30).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", type=Path, required=True)
    args = parser.parse_args()
    worker = args.worker.resolve()
    for tool in ("cc", "objcopy", "objdump", "nm"):
        if not shutil.which(tool):
            parser.error(f"Missing controlled-fixture prerequisite: {tool}")
    version = json.loads(checked([worker, "--version"]))
    assert version["protocol"] == PROTOCOL
    assert version["backend"]["revision"] == "c4273522017788fb67c30058ffd5bbdf291fcc40"
    simple = {"protocol": PROTOCOL, "architecture": ARCHITECTURE, "entry": "0x1000", "name": "add_one",
              "segments": [{"address": "0x1000", "bytes_hex": "8d4701c3", "readonly": True}],
              "functions": [{"address": "0x1000", "name": "add_one", "return_type": "uint32",
                             "parameters": [{"name": "value", "type": "uint32"}]}]}
    response = run(worker, simple)
    assert "uint4 add_one(uint4 value)" in response["pseudocode"]
    for text, address in (("+", "0x1000"), ("return", "0x1003")):
        assert any(token["text"] == text and token["address"] == address for token in response["tokens"])
    for change in ({"architecture": "AARCH64:LE:64:v8A"}, {"entry": "0x2000"},
                   {"name": "bad;name"}, {"segments": simple["segments"] * 2}):
        invalid = copy.deepcopy(simple)
        invalid.update(change)
        run(worker, invalid, success=False)
    invalid = copy.deepcopy(simple)
    invalid["functions"][0]["return_type"] = "imaginary"
    run(worker, invalid, success=False)
    with tempfile.TemporaryDirectory(prefix="ale-native-accept-") as temporary:
        root = Path(temporary)
        source, executable, raw = root / "fixture.c", root / "fixture.elf", root / "text.bin"
        source.write_text("__attribute__((noinline)) int helper(int value) { return value * 3; }\n"
                          "__attribute__((noinline)) int choose(int value) {\n"
                          "  if (value < 0) return helper(-value) + 5;\n"
                          "  if (value == 4) return 7;\n"
                          "  return helper(value) + 2;\n}\n"
                          "int main(void) { return choose(4); }\n")
        checked(["cc", "-O1", "-fno-pie", "-no-pie", "-fno-stack-protector", "-fno-asynchronous-unwind-tables", source, "-o", executable])
        checked(["objcopy", "-O", "binary", "--only-section=.text", executable, raw])
        sections = checked(["objdump", "-h", executable])
        match = re.search(r"^\s*\d+\s+\.text\s+([0-9a-f]+)\s+([0-9a-f]+)", sections, re.M)
        assert match, sections
        size, base = (int(value, 16) for value in match.groups())
        text_bytes = raw.read_bytes()
        assert size == len(text_bytes)
        symbols = {name: int(address, 16) for address, name in re.findall(r"^([0-9a-f]+) T (helper|choose)$", checked(["nm", "-n", executable]), re.M)}
        request = {"protocol": PROTOCOL, "architecture": ARCHITECTURE, "entry": hex(symbols["choose"]), "name": "choose",
                   "segments": [{"address": hex(base), "bytes_hex": text_bytes.hex(), "readonly": True}],
                   "functions": [{"address": hex(address), "name": name, "return_type": "int32",
                                  "parameters": [{"name": "value", "type": "int32"}]} for name, address in symbols.items()]}
        response = run(worker, request)
        pseudocode = response["pseudocode"]
        assert "int4 choose(int4 value)" in pseudocode and "if (" in pseudocode and "helper(" in pseudocode, pseudocode
        disassembly = checked(["objdump", "-d", "--disassemble=choose", executable])
        call_addresses = {int(match, 16) for match in re.findall(r"^\s*([0-9a-f]+):.*\bcall\s", disassembly, re.M)}
        mapped_calls = {int(token["address"], 16) for token in response["tokens"] if token["text"] == "helper" and token["address"]}
        assert mapped_calls and mapped_calls <= call_addresses, (response, disassembly)
        instruction_addresses = {int(match, 16) for match in re.findall(r"^\s*([0-9a-f]+):", disassembly, re.M)}
        assert all(int(token["address"], 16) in instruction_addresses for token in response["tokens"] if token["address"])
        print(pseudocode)
        print("Independent objdump confirmed call-token instruction addresses:", ", ".join(hex(address) for address in sorted(mapped_calls)))
        inferred = copy.deepcopy(request)
        inferred.pop("functions")
        assert run(worker, inferred)["pseudocode"].strip()
    print("PASS: native typed pseudocode, conditional/calls, token offsets/address mappings, untyped input, and recoverable rejection cases. No fixture was executed.")


if __name__ == "__main__":
    main()
