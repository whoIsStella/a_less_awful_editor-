#!/usr/bin/env python3
"""One-shot JSON adapter for the optional native decompiler; no target execution."""
import argparse
import json
import os
from pathlib import Path
import re
import resource
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

PROTOCOL = "ale-decompiler-v1"
REVISION = "c4273522017788fb67c30058ffd5bbdf291fcc40"
ARCHITECTURE = "x86:LE:64:default:gcc"
BACKEND = {"name": "ghidra-native", "revision": REVISION}
MAX_BYTES = 16 * 1024 * 1024
MAX_REQUEST = MAX_BYTES * 2 + 1024 * 1024
MAX_OUTPUT = 8 * 1024 * 1024
TYPES = {"void", "bool"} | {f"{prefix}{bits}" for prefix in ("int", "uint") for bits in (8, 16, 32, 64)}


def address(value):
    if not isinstance(value, str) or not re.fullmatch(r"0x[0-9a-fA-F]{1,16}", value):
        raise ValueError("Addresses must be hexadecimal strings, e.g. 0x401000")
    return int(value, 16)


def identifier(value):
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]{0,127}", value):
        raise ValueError("Function and parameter names must be ASCII C identifiers of at most 128 characters")
    return value


def request_xml(request):
    if not isinstance(request, dict) or request.get("protocol") != PROTOCOL:
        raise ValueError("Unsupported or missing protocol")
    if request.get("architecture") != ARCHITECTURE:
        raise ValueError(f"Unsupported architecture; currently requires {ARCHITECTURE}")
    entry = address(request.get("entry"))
    name = identifier(request.get("name", f"function_{entry:x}"))
    root = ET.Element("request", entry=hex(entry), name=name)
    image = ET.SubElement(root, "binaryimage", arch=ARCHITECTURE)
    segments = request.get("segments")
    if not isinstance(segments, list) or not 1 <= len(segments) <= 256:
        raise ValueError("Provide between 1 and 256 mapped segments")
    ranges = []
    total = 0
    for segment in segments:
        if not isinstance(segment, dict):
            raise ValueError("Invalid mapped segment")
        start = address(segment.get("address"))
        encoded = segment.get("bytes_hex")
        if not isinstance(encoded, str) or not encoded or len(encoded) % 2 or not re.fullmatch(r"[0-9a-fA-F]+", encoded):
            raise ValueError("Segment bytes_hex must contain complete hexadecimal bytes")
        size = len(encoded) // 2
        total += size
        if total > MAX_BYTES or start + size > 2**64:
            raise ValueError("Mapped snapshot exceeds size or address limit")
        end = start + size
        if any(start < previous_end and previous_start < end for previous_start, previous_end in ranges):
            raise ValueError("Overlapping mapped segments are not supported")
        ranges.append((start, end))
        readonly = segment.get("readonly", False)
        if not isinstance(readonly, bool):
            raise ValueError("readonly must be boolean")
        ET.SubElement(image, "bytechunk", space="ram", offset=hex(start), readonly=str(readonly).lower()).text = encoded
    if not any(start <= entry < end for start, end in ranges):
        raise ValueError("Function entry is outside mapped snapshot")
    functions = request.get("functions", [])
    if not isinstance(functions, list) or len(functions) > 4096:
        raise ValueError("At most 4096 function metadata records are supported")
    known = set()
    names = set()
    for function in functions:
        if not isinstance(function, dict):
            raise ValueError("Invalid function metadata")
        offset = address(function.get("address"))
        function_name = identifier(function.get("name"))
        if offset in known or function_name in names:
            raise ValueError("Duplicate function address or name")
        known.add(offset)
        names.add(function_name)
        typed = "return_type" in function or "parameters" in function
        node = ET.SubElement(root, "function", address=hex(offset), name=function_name, typed=str(typed).lower())
        if typed:
            result = function.get("return_type")
            parameters = function.get("parameters")
            if result not in TYPES or not isinstance(parameters, list) or len(parameters) > 64:
                raise ValueError("Typed functions require supported return_type and at most 64 parameters")
            node.set("return_type", result)
            parameter_names = set()
            for parameter in parameters:
                if not isinstance(parameter, dict) or parameter.get("type") not in TYPES - {"void"}:
                    raise ValueError("Unsupported parameter type")
                parameter_name = identifier(parameter.get("name"))
                if parameter_name in parameter_names:
                    raise ValueError("Duplicate parameter name")
                parameter_names.add(parameter_name)
                ET.SubElement(node, "parameter", name=parameter_name, type=parameter["type"])
    return ET.tostring(root, encoding="utf-8"), ranges


def parse_result(data, request, ranges):
    result = ET.fromstring(data)
    if result.tag != "result":
        raise ValueError("Unexpected native response")
    operations = {int(node.attrib["id"]): node.attrib["address"] for node in result.findall("./operations/op")}
    function = result.find("./markup/function")
    if function is None:
        raise ValueError("Native response contains no pseudocode")
    parts = []
    tokens = []
    position = 0
    for node in function.iter():
        if node.tag == "break":
            text = "\n" + " " * int(node.attrib.get("indent", "0"))
        elif not len(node):
            text = node.text or ""
        else:
            continue
        if not text:
            continue
        size = len(text.encode("utf-8"))
        mapped = operations.get(int(node.attrib["opref"], 0)) if "opref" in node.attrib else None
        if mapped and not any(start <= address(mapped) < end for start, end in ranges):
            mapped = None
        if node.tag != "break":
            tokens.append({"start": position, "end": position + size, "kind": node.tag, "text": text, "address": mapped})
        parts.append(text)
        position += size
    pseudocode = "".join(parts)
    if not pseudocode.strip():
        raise ValueError("Native response contains empty pseudocode")
    diagnostics = [node.text.strip() for node in result.findall("diagnostic") if node.text and node.text.strip()]
    return {"protocol": PROTOCOL, "ok": True, "entry": request["entry"], "backend": BACKEND,
            "pseudocode": pseudocode, "tokens": tokens, "diagnostics": diagnostics}


def limits():
    resource.setrlimit(resource.RLIMIT_CPU, (15, 15))
    resource.setrlimit(resource.RLIMIT_AS, (2 * 1024**3, 2 * 1024**3))
    resource.setrlimit(resource.RLIMIT_FSIZE, (MAX_OUTPUT, MAX_OUTPUT))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", type=Path)
    parser.add_argument("--languages", type=Path)
    parser.add_argument("--version", action="store_true")
    args = parser.parse_args()
    if args.version:
        print(json.dumps({"protocol": PROTOCOL, "backend": BACKEND, "architectures": [ARCHITECTURE]}))
        return 0
    try:
        raw = sys.stdin.buffer.read(MAX_REQUEST + 1)
        if len(raw) > MAX_REQUEST:
            raise ValueError("Request exceeds input limit")
        request = json.loads(raw)
        encoded, ranges = request_xml(request)
        if not args.backend or not args.languages:
            raise ValueError("Native backend is not configured; use the generated launcher")
        env = {"PATH": "/nonexistent", "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8"}
        # Keep the child in this worker's process group so the IDE can cancel
        # the entire request. Temporary output files cap native pipe growth.
        with tempfile.TemporaryDirectory(prefix="ale-decompile-") as directory:
            with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
                result = subprocess.run([str(args.backend), str(args.languages)], input=encoded,
                    stdout=output, stderr=errors, cwd=directory, env=env, timeout=15, preexec_fn=limits)
                output.seek(0)
                errors.seek(0)
                error_text = errors.read(MAX_OUTPUT).decode("utf-8", errors="replace")
                if result.returncode:
                    raise ValueError(f"Native decompiler exited {result.returncode}: {error_text[:4000]}")
                data = output.read(MAX_OUTPUT + 1)
                if len(data) > MAX_OUTPUT:
                    raise ValueError("Native output exceeded limit")
        response = parse_result(data, request, ranges)
        if error_text.strip():
            response["diagnostics"].append(error_text.strip()[:4000])
    except (ValueError, KeyError, TypeError, OSError, ET.ParseError, subprocess.TimeoutExpired) as error:
        response = {"protocol": PROTOCOL, "ok": False, "error": str(error), "backend": BACKEND}
    print(json.dumps(response, ensure_ascii=True))
    return 0 if response["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
