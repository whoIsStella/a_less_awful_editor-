#!/usr/bin/env python3
"""One-shot JSON adapter for the optional native decompiler; no target execution."""
import argparse
from bisect import bisect_right
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
# MAX_OUTPUT bounds both native XML and the complete newline-terminated JSON
# frame. Bounding only native bytes does not bound reconstructed indentation or
# JSON escaping/token metadata. The IDE runner must enforce this limit too.
MAX_OUTPUT = 8 * 1024 * 1024
MAX_PSEUDOCODE_BYTES = 1024 * 1024
MAX_TOKENS = 65_536
MAX_OPERATIONS = 131_072
MAX_XML_NODES = 262_144
MAX_XML_DEPTH = 128
MAX_INDENT = 256
MAX_DIAGNOSTICS = 64
MAX_DIAGNOSTIC_CHARS = 4_000
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
            if (not isinstance(result, str) or result not in TYPES
                    or not isinstance(parameters, list) or len(parameters) > 64):
                raise ValueError("Typed functions require supported return_type and at most 64 parameters")
            node.set("return_type", result)
            parameter_names = set()
            for parameter in parameters:
                if (not isinstance(parameter, dict)
                        or not isinstance(parameter.get("type"), str)
                        or parameter["type"] not in TYPES - {"void"}):
                    raise ValueError("Unsupported parameter type")
                parameter_name = identifier(parameter.get("name"))
                if parameter_name in parameter_names:
                    raise ValueError("Duplicate parameter name")
                parameter_names.add(parameter_name)
                ET.SubElement(node, "parameter", name=parameter_name, type=parameter["type"])
    return ET.tostring(root, encoding="utf-8"), ranges


def load_request(raw):
    """Accept one bounded UTF-8 JSON document without ambiguous duplicate keys."""
    if len(raw) > MAX_REQUEST:
        raise ValueError("Request exceeds input limit")

    def unique_object(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise ValueError("Duplicate JSON object key")
            value[key] = item
        return value

    def reject_constant(_):
        raise ValueError("Non-finite numbers are not valid protocol JSON")

    return json.loads(raw.decode("utf-8"), object_pairs_hook=unique_object,
                      parse_constant=reject_constant)


def encode_response(response):
    """Construct the entire wire frame before writing any success response."""
    encoded = (json.dumps(response, ensure_ascii=True, allow_nan=False,
                          separators=(",", ":")) + "\n").encode("ascii")
    if len(encoded) > MAX_OUTPUT:
        raise ValueError("Serialized response exceeds output limit")
    return encoded


def native_xml(data):
    if not isinstance(data, bytes) or len(data) > MAX_OUTPUT:
        raise ValueError("Native response must be bounded UTF-8 XML")
    text = data.decode("utf-8")
    # The private native protocol needs no DTD/entities or alternative encodings.
    # Reject NUL as well so UTF-16 bytes cannot evade this pre-parse check.
    if "\x00" in text or re.search(r"<!\s*(?:DOCTYPE|ENTITY)\b", text, re.I):
        raise ValueError("DTD, entity declarations and NUL bytes are not supported")
    declaration = re.match(r"\ufeff?\s*<\?xml\b.*?\?>", text, re.S)
    if declaration:
        encoding = re.search(r"encoding\s*=\s*['\"]([^'\"]+)['\"]", declaration[0])
        if encoding and encoding[1].lower() not in {"utf-8", "utf8"}:
            raise ValueError("Native response must use UTF-8")
    parser = ET.XMLPullParser(events=("start", "end"))
    root = None
    depth = count = 0
    for offset in range(0, len(text), 16_384):
        parser.feed(text[offset:offset + 16_384])
        for event, node in parser.read_events():
            if event == "start":
                count += 1
                depth += 1
                if root is None:
                    root = node
                if count > MAX_XML_NODES or depth > MAX_XML_DEPTH:
                    raise ValueError("Native XML exceeds structure limit")
            else:
                depth -= 1
    parser.close()
    if root is None or root.tag != "result":
        raise ValueError("Unexpected native response")
    return root


def operation_id(value):
    if not isinstance(value, str) or not re.fullmatch(
            r"(?:0x[0-9a-fA-F]{1,16}|[0-9]{1,20})", value):
        raise ValueError("Invalid native operation reference")
    result = int(value, 16 if value.startswith("0x") else 10)
    if result >= 2**64:
        raise ValueError("Native operation reference overflows")
    return result


def add_diagnostic(diagnostics, text):
    if text and text.strip():
        if len(diagnostics) >= MAX_DIAGNOSTICS:
            raise ValueError("Native diagnostics exceed count limit")
        diagnostics.append(text.strip()[:MAX_DIAGNOSTIC_CHARS])


def parse_result(data, request, ranges):
    result = native_xml(data)
    if any(node.tag not in {"markup", "operations", "diagnostic"} for node in result):
        raise ValueError("Unexpected native response section")
    markups = result.findall("markup")
    operation_lists = result.findall("operations")
    if (len(markups) != 1 or len(markups[0]) != 1
            or markups[0][0].tag != "function" or len(operation_lists) != 1):
        raise ValueError("Native response must contain exactly one pseudocode function and operation list")
    if len(operation_lists[0]) > MAX_OPERATIONS:
        raise ValueError("Native operations exceed count limit")
    mapped_ranges = sorted(ranges)
    starts = [start for start, _ in mapped_ranges]
    operations = {}
    for node in operation_lists[0]:
        if node.tag != "op":
            raise ValueError("Unexpected native operation element")
        reference = operation_id(node.attrib.get("id"))
        if reference in operations:
            raise ValueError("Duplicate native operation reference")
        location = address(node.attrib.get("address"))
        index = bisect_right(starts, location) - 1
        mapped = index >= 0 and location < mapped_ranges[index][1]
        # Preserve absent mappings instead of claiming an external/native address
        # belongs to this snapshot. This is not proof of instruction boundaries.
        operations[reference] = hex(location) if mapped else None
    parts = []
    tokens = []
    position = 0
    for node in markups[0][0].iter():
        if node.tag == "break":
            indentation = node.attrib.get("indent", "0")
            if (not re.fullmatch(r"[0-9]{1,4}", indentation)
                    or int(indentation) > MAX_INDENT):
                raise ValueError("Native indentation exceeds limit or is invalid")
            text = "\n" + " " * int(indentation)
        elif not len(node):
            text = node.text or ""
        else:
            continue
        if not text:
            continue
        size = len(text.encode("utf-8"))
        if position + size > MAX_PSEUDOCODE_BYTES:
            raise ValueError("Reconstructed pseudocode exceeds byte limit")
        mapped = operations.get(operation_id(node.attrib["opref"])) if "opref" in node.attrib else None
        if node.tag != "break":
            if len(tokens) >= MAX_TOKENS or len(node.tag) > 128:
                raise ValueError("Native tokens exceed count or kind limit")
            tokens.append({"start": position, "end": position + size, "kind": node.tag,
                           "text": text, "address": mapped})
        parts.append(text)
        position += size
    pseudocode = "".join(parts)
    if not pseudocode.strip():
        raise ValueError("Native response contains empty pseudocode")
    diagnostics = []
    for node in result.findall("diagnostic"):
        add_diagnostic(diagnostics, node.text)
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
        sys.stdout.buffer.write(encode_response(
            {"protocol": PROTOCOL, "backend": BACKEND, "architectures": [ARCHITECTURE]}))
        return 0
    try:
        raw = sys.stdin.buffer.read(MAX_REQUEST + 1)
        request = load_request(raw)
        encoded, ranges = request_xml(request)
        if not args.backend or not args.languages:
            raise ValueError("Native backend is not configured; use the generated launcher")
        # Resolve explicit configuration before changing the native child's cwd.
        backend = args.backend.resolve(strict=True)
        languages = args.languages.resolve(strict=True)
        if not backend.is_file() or not languages.is_dir():
            raise ValueError("Backend must be a file and languages must be a directory")
        env = {"PATH": "/nonexistent", "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8"}
        # Keep the child in this worker's process group so the IDE can cancel
        # the entire request. Temporary output files cap native pipe growth.
        with tempfile.TemporaryDirectory(prefix="ale-decompile-") as directory:
            with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
                result = subprocess.run([str(backend), str(languages)], input=encoded,
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
        add_diagnostic(response["diagnostics"], error_text)
        frame = encode_response(response)
    except (ValueError, KeyError, TypeError, OSError, RecursionError,
            ET.ParseError, subprocess.SubprocessError) as error:
        response = {"protocol": PROTOCOL, "ok": False,
                    "error": str(error)[:MAX_DIAGNOSTIC_CHARS], "backend": BACKEND}
        frame = encode_response(response)
    sys.stdout.buffer.write(frame)
    return 0 if response["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
