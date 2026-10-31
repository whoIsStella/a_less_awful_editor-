#!/usr/bin/env python3
"""Dependency-free adapter contract tests. No Ghidra or target program is run."""
import importlib.util
import json
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

WORKER_PATH = Path(__file__).with_name("worker.py").resolve()
SPEC = importlib.util.spec_from_file_location("ale_worker_under_test", WORKER_PATH)
worker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(worker)


def request():
    return {
        "protocol": worker.PROTOCOL,
        "architecture": worker.ARCHITECTURE,
        "entry": "0x1000",
        "name": "add_one",
        "segments": [{"address": "0x1000", "bytes_hex": "8d4701c3", "readonly": True}],
        "functions": [{"address": "0x1000", "name": "add_one", "return_type": "uint32",
                       "parameters": [{"name": "value", "type": "uint32"}]}],
    }


def result_xml(markup='<break indent="0"/><keyword opref="1">return</keyword>'
               '<syntax> </syntax><variable opref="0x2">café😀</variable><syntax>;</syntax>',
               operations='<op id="1" address="0x1003"/><op id="2" address="0x1000"/>',
               diagnostics=""):
    return (f"<result><markup><function>{markup}</function></markup>"
            f"<operations>{operations}</operations>{diagnostics}</result>").encode("utf-8")


def parse(data):
    return worker.parse_result(data, request(), [(0x1000, 0x1004)])


class RequestContract(unittest.TestCase):
    def test_valid_typed_snapshot_becomes_native_xml(self):
        data, ranges = worker.request_xml(request())
        root = ET.fromstring(data)
        self.assertEqual(ranges, [(0x1000, 0x1004)])
        self.assertEqual(root.attrib["entry"], "0x1000")
        self.assertEqual(root.find("./binaryimage/bytechunk").text, "8d4701c3")
        self.assertEqual(root.find("./function/parameter").attrib, {"name": "value", "type": "uint32"})

    def test_rejects_missing_or_wrong_protocol_and_architecture(self):
        for fields in ({"protocol": None}, {"protocol": "future"}, {"architecture": "arm"}):
            with self.subTest(fields=fields), self.assertRaises(ValueError):
                worker.request_xml(request() | fields)

    def test_rejects_malformed_or_unmapped_entry(self):
        for value in (None, 4096, "4096", "-0x1", "0x10000000000000000", "0x2000"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                worker.request_xml(request() | {"entry": value})

    def test_rejects_invalid_segment_encodings(self):
        for value in (None, "", "f", "ff zz", "0x90", ["90"]):
            item = request()
            item["segments"][0]["bytes_hex"] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                worker.request_xml(item)

    def test_rejects_overlap_and_accepts_adjacent_segments(self):
        item = request()
        item["segments"].append({"address": "0x1003", "bytes_hex": "c3"})
        with self.assertRaises(ValueError):
            worker.request_xml(item)
        item["segments"][1]["address"] = "0x1004"
        self.assertEqual(worker.request_xml(item)[1], [(0x1000, 0x1004), (0x1004, 0x1005)])

    def test_rejects_address_overflow(self):
        item = request() | {"entry": "0xffffffffffffffff", "functions": []}
        item["segments"] = [{"address": item["entry"], "bytes_hex": "90c3"}]
        with self.assertRaises(ValueError):
            worker.request_xml(item)

    def test_enforces_segment_count_and_byte_budget(self):
        for segments in ([], request()["segments"] * 257):
            with self.subTest(count=len(segments)), self.assertRaises(ValueError):
                worker.request_xml(request() | {"segments": segments})
        with patch.object(worker, "MAX_BYTES", 3), self.assertRaises(ValueError):
            worker.request_xml(request())

    def test_requires_boolean_readonly_flag(self):
        item = request()
        item["segments"][0]["readonly"] = 1
        with self.assertRaises(ValueError):
            worker.request_xml(item)

    def test_rejects_duplicate_functions_and_parameters(self):
        item = request()
        item["functions"] *= 2
        with self.assertRaises(ValueError):
            worker.request_xml(item)
        item = request()
        item["functions"][0]["parameters"] *= 2
        with self.assertRaises(ValueError):
            worker.request_xml(item)

    def test_rejects_unsupported_and_unhashable_type_metadata(self):
        for value in ("imaginary", [], {}):
            item = request()
            item["functions"][0]["return_type"] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                worker.request_xml(item)

    def test_untyped_function_and_missing_metadata_are_allowed(self):
        item = request()
        item["functions"] = [{"address": "0x1000", "name": "add_one"}]
        data, _ = worker.request_xml(item)
        self.assertEqual(ET.fromstring(data).find("function").attrib["typed"], "false")
        item.pop("functions")
        self.assertIsNotNone(worker.request_xml(item))


    def test_request_byte_limit_is_checked_before_decoding(self):
        with patch.object(worker, "MAX_REQUEST", 3), self.assertRaises(ValueError):
            worker.load_request(b"not json")

    def test_request_encoding_is_strict_utf8(self):
        with self.assertRaises(ValueError):
            worker.load_request(b"\xff")

    def test_void_is_not_a_valid_parameter_type(self):
        item = request()
        item["functions"][0]["parameters"][0]["type"] = "void"
        with self.assertRaises(ValueError):
            worker.request_xml(item)


class ResponseContract(unittest.TestCase):
    def test_unicode_tokens_are_byte_ranges_in_exact_pseudocode(self):
        result = parse(result_xml())
        self.assertEqual(result["pseudocode"], "\nreturn café😀;")
        self.assertEqual(result["tokens"][0]["address"], "0x1003")
        text = result["pseudocode"].encode("utf-8")
        end = 0
        for token in result["tokens"]:
            self.assertLessEqual(end, token["start"])
            self.assertLess(token["start"], token["end"])
            self.assertEqual(text[token["start"]:token["end"]].decode(), token["text"])
            end = token["end"]

    def test_unknown_operation_reference_stays_unmapped(self):
        result = parse(result_xml(markup='<variable opref="999">value</variable>'))
        self.assertIsNone(result["tokens"][0]["address"])

    def test_outside_snapshot_operation_stays_unmapped(self):
        result = parse(result_xml(operations='<op id="1" address="0x2000"/>'))
        self.assertIsNone(result["tokens"][0]["address"])

    def test_rejects_duplicate_operation_ids(self):
        with self.assertRaises(ValueError):
            parse(result_xml(operations='<op id="1" address="0x1000"/><op id="1" address="0x1003"/>'))

    def test_rejects_invalid_operation_ids_and_addresses(self):
        for operations in ('<op id="-1" address="0x1000"/>',
                           '<op id="18446744073709551616" address="0x1000"/>',
                           '<op id="9" address="not-an-address"/>'):
            with self.subTest(operations=operations), self.assertRaises(ValueError):
                parse(result_xml(operations=operations))

    def test_rejects_invalid_oprefs(self):
        with self.assertRaises(ValueError):
            parse(result_xml(markup='<keyword opref="-1">return</keyword>'))

    def test_rejects_negative_excessive_and_invalid_indentation(self):
        # Small fixtures prove the guard without constructing an actual huge allocation.
        for indent in ("-1", "257", "abc"):
            with self.subTest(indent=indent), self.assertRaises(ValueError):
                parse(result_xml(markup=f'<break indent="{indent}"/><syntax>x</syntax>'))

    def test_pseudocode_budget_includes_expanded_indentation(self):
        with patch.object(worker, "MAX_PSEUDOCODE_BYTES", 16, create=True), self.assertRaises(ValueError):
            parse(result_xml(markup='<break indent="32"/><syntax>x</syntax>'))

    def test_pseudocode_budget_counts_utf8_bytes(self):
        with patch.object(worker, "MAX_PSEUDOCODE_BYTES", 3, create=True), self.assertRaises(ValueError):
            parse(result_xml(markup='<variable>😀</variable>'))

    def test_enforces_native_xml_size(self):
        with patch.object(worker, "MAX_OUTPUT", 32), self.assertRaises(ValueError):
            parse(result_xml())

    def test_enforces_token_and_operation_counts(self):
        with patch.object(worker, "MAX_TOKENS", 1, create=True), self.assertRaises(ValueError):
            parse(result_xml())
        with patch.object(worker, "MAX_OPERATIONS", 1, create=True), self.assertRaises(ValueError):
            parse(result_xml())

    def test_rejects_dtd_and_entity_declarations(self):
        data = (b'<!DOCTYPE result [<!ENTITY word "return">]>' +
                result_xml(markup='<syntax>&word;</syntax>'))
        with self.assertRaises(ValueError):
            parse(data)

    def test_rejects_deep_and_excessive_xml_structure(self):
        with patch.object(worker, "MAX_XML_DEPTH", 4, create=True), self.assertRaises(ValueError):
            parse(result_xml(markup='<a><b><c>x</c></b></a>'))
        with patch.object(worker, "MAX_XML_NODES", 4, create=True), self.assertRaises(ValueError):
            parse(result_xml())

    def test_rejects_multiple_pseudocode_functions(self):
        data = result_xml().replace(b'</markup>', b'<function><syntax>x</syntax></function></markup>')
        with self.assertRaises(ValueError):
            parse(data)

    def test_rejects_unexpected_top_level_sections(self):
        data = result_xml().replace(b'</result>', b'<unexpected/></result>')
        with self.assertRaises(ValueError):
            parse(data)

    def test_empty_and_malformed_results_are_errors(self):
        for data in (b'<result/>', b'<result>', result_xml(markup='<break/>')):
            with self.subTest(data=data), self.assertRaises((ValueError, ET.ParseError)):
                parse(data)

    def test_native_diagnostics_have_bounded_size_and_count(self):
        with patch.object(worker, "MAX_DIAGNOSTIC_CHARS", 8, create=True):
            result = parse(result_xml(diagnostics='<diagnostic>' + 'x' * 32 + '</diagnostic>'))
            self.assertEqual(result["diagnostics"], ['x' * 8])
        with patch.object(worker, "MAX_DIAGNOSTICS", 1, create=True), self.assertRaises(ValueError):
            parse(result_xml(diagnostics='<diagnostic>a</diagnostic><diagnostic>b</diagnostic>'))

    def test_serialized_response_has_a_final_wire_limit(self):
        response = parse(result_xml())
        encoded = worker.encode_response(response)
        self.assertTrue(encoded.endswith(b'\n'))
        self.assertEqual(json.loads(encoded), response)
        with patch.object(worker, "MAX_OUTPUT", len(encoded) - 1), self.assertRaises(ValueError):
            worker.encode_response(response)


    def test_utf16_cannot_bypass_the_dtd_guard(self):
        data = ('<!DOCTYPE result [<!ENTITY word "return">]>' +
                result_xml(markup='<syntax>&word;</syntax>').decode()).encode("utf-16-le")
        with self.assertRaises(ValueError):
            parse(data)

    def test_rejects_encoding_declarations_other_than_utf8(self):
        with self.assertRaises(ValueError):
            parse(b'<?xml version="1.0" encoding="iso-8859-1"?>' + result_xml())

    def test_seeded_markup_round_trips_exact_bytes(self):
        generator = random.Random(270927)
        alphabet = ("value", " ", "😀", "e\u0301", "<&>", "日本語", "\t")
        for _ in range(200):
            elements = []
            expected = []
            for _ in range(generator.randint(1, 24)):
                if generator.randrange(5) == 0:
                    indentation = generator.randrange(9)
                    elements.append(ET.Element("break", indent=str(indentation)))
                    expected.append("\n" + " " * indentation)
                else:
                    element = ET.Element("syntax", opref="2")
                    element.text = generator.choice(alphabet)
                    elements.append(element)
                    expected.append(element.text)
            markup = "".join(ET.tostring(element, encoding="unicode") for element in elements)
            if not "".join(expected).strip():
                with self.assertRaises(ValueError):
                    parse(result_xml(markup=markup))
                continue
            response = parse(result_xml(markup=markup))
            self.assertEqual(response["pseudocode"], "".join(expected))
            text = response["pseudocode"].encode()
            for token in response["tokens"]:
                self.assertEqual(text[token["start"]:token["end"]].decode(), token["text"])
                self.assertEqual(token["address"], "0x1000")


class AdapterProcess(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="ale-protocol-test-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.backend = self.root / "fake native backend.py"
        self.backend.write_text(
            '#!' + sys.executable + '\nimport sys\nsys.stdin.buffer.read()\n'
            'sys.stdout.buffer.write(' + repr(result_xml()) + ')\n', encoding="utf-8")
        self.backend.chmod(0o700)

    def invoke(self, raw, backend=None):
        result = subprocess.run(
            [sys.executable, str(WORKER_PATH), '--backend', str(backend or self.backend),
             '--languages', str(self.root)], input=raw, capture_output=True,
            cwd=self.root, timeout=5, check=False,
        )
        self.assertTrue(result.stdout, result.stderr)
        response = json.loads(result.stdout)
        self.assertEqual(response['protocol'], worker.PROTOCOL)
        self.assertEqual(result.returncode == 0, response['ok'])
        return response, result

    def test_json_adapter_round_trip_with_fake_backend(self):
        response, result = self.invoke(json.dumps(request()).encode())
        self.assertTrue(response['ok'])
        self.assertEqual(response['pseudocode'], '\nreturn café😀;')
        self.assertEqual(result.stderr, b'')

    def test_relative_backend_path_is_resolved_before_temporary_cwd(self):
        response, _ = self.invoke(json.dumps(request()).encode(), Path(self.backend.name))
        self.assertTrue(response['ok'], response)

    def test_duplicate_json_keys_are_rejected(self):
        data = json.dumps(request()).replace('"entry": "0x1000"', '"entry": "0x2000", "entry": "0x1000"')
        response, _ = self.invoke(data.encode())
        self.assertFalse(response['ok'])

    def test_nonfinite_json_numbers_are_rejected(self):
        item = request() | {'extension': float('nan')}
        response, _ = self.invoke(json.dumps(item).encode())
        self.assertFalse(response['ok'])

    def test_deeply_nested_json_returns_protocol_error_not_traceback(self):
        response, result = self.invoke(('[' * 2_000 + '0' + ']' * 2_000).encode())
        self.assertFalse(response['ok'])
        self.assertNotIn(b'Traceback', result.stderr)

    def test_backend_failure_retains_bounded_protocol_error(self):
        self.backend.write_text('#!' + sys.executable + '\nimport sys\nsys.stderr.write("failed"*2000)\nsys.exit(7)\n')
        response, _ = self.invoke(json.dumps(request()).encode())
        self.assertFalse(response['ok'])
        self.assertLessEqual(len(response['error']), 4_000)

    def test_version_handshake_requires_no_native_backend(self):
        result = subprocess.run([sys.executable, str(WORKER_PATH), '--version'],
                                capture_output=True, timeout=5, check=True)
        response = json.loads(result.stdout)
        self.assertEqual(response['protocol'], worker.PROTOCOL)
        self.assertEqual(response['backend']['revision'], worker.REVISION)
        self.assertEqual(response['architectures'], [worker.ARCHITECTURE])


    def test_malformed_backend_xml_is_a_protocol_error(self):
        self.backend.write_text('#!' + sys.executable + '\nprint("<result>")\n')
        response, _ = self.invoke(json.dumps(request()).encode())
        self.assertFalse(response["ok"])

    def test_nonexecutable_backend_is_a_protocol_error(self):
        self.backend.chmod(0o600)
        response, _ = self.invoke(json.dumps(request()).encode())
        self.assertFalse(response["ok"])

    def test_missing_language_directory_is_a_protocol_error(self):
        result = subprocess.run(
            [sys.executable, str(WORKER_PATH), "--backend", str(self.backend),
             "--languages", str(self.root / "missing")],
            input=json.dumps(request()).encode(), capture_output=True, timeout=5,
        )
        self.assertEqual(result.returncode, 1)
        self.assertFalse(json.loads(result.stdout)["ok"])

    def test_native_output_file_limit_is_enforced(self):
        self.backend.write_text(
            '#!' + sys.executable + '\nimport sys\nsys.stdin.buffer.read()\n'
            'sys.stdout.buffer.write(b"x" * ' + str(worker.MAX_OUTPUT + 1) + ')\n')
        response, result = self.invoke(json.dumps(request()).encode())
        self.assertFalse(response["ok"])
        self.assertLessEqual(len(result.stdout), worker.MAX_OUTPUT)


if __name__ == '__main__':
    unittest.main()
