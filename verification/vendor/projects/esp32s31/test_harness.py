"""Host regressions for request preparation; no hardware or private inputs."""
import copy
import pathlib
import tempfile
import unittest
from harness import Buffer, ProbeCatalog, Runner, abi_word, case, invocation, region, words, single_argument_entry


class HarnessTests(unittest.TestCase):
    def test_signed_words_and_explicit_padding(self):
        self.assertEqual(abi_word("i8", -128), 0xffffff80)
        self.assertEqual(abi_word("u16", 65535), 65535)
        self.assertIsNone(abi_word("u32", None))
        self.assertEqual(abi_word("bool", True), 1)
        self.assertEqual(abi_word("bool", False), 0)
        self.assertEqual(words([7], pad_to=2, fill=0), [7, 0, 0, 0, 0, 0, 0, 0])
        for kind, value in [("i8", 128), ("u32", -1), ("u64", 1), ("Private", 0), ("bool", 2)]:
            with self.assertRaises(ValueError):
                abi_word(kind, value)
        with self.assertRaises(ValueError):
            words([1], pad_to=8)
        with self.assertRaises(ValueError):
            words([1, 2], pad_to=1, fill=0)

    def test_phase_building_preserves_unknown_memory_and_isolates_mutation(self):
        memory = region(0x1000, 4, [1], lifetime="session")
        self.assertIsNone(memory["seed"]["fill"])
        left = invocation(10, [None], [memory])
        right = invocation(20, [None], [memory])
        row = case("warm", left, right, reset="warm", memory=True)
        row["vendor"]["memory"][0]["seed"]["bytes"][0] = 2
        self.assertEqual(right["memory"][0]["seed"]["bytes"], [1])
        self.assertEqual(row["reset"], "warm")
        self.assertEqual(row["relation"]["memory"], [dict(vendor=0, replacement=0)])
        with self.assertRaises(ValueError):
            region(0xffffffff, 4)
        with self.assertRaises(ValueError):
            region(0, 1, [1, 2])

    def test_catalog_requires_unique_declared_entries_and_exact_arguments(self):
        declaration = dict(symbol="sample", adapter="call", signature='extern "C" fn sample(a: i8)',
                           arguments=[dict(name="a", rust_type="i8")])
        catalog = dict(schema=1, image="fixture", entries=[declaration])
        probes = ProbeCatalog(catalog, lambda name: dict(value=32))
        self.assertEqual(probes.entry("sample")["value"], 32)
        self.assertEqual(probes.arguments("sample", dict(a=-1)), [0xffffffff])
        with self.assertRaises(ValueError):
            probes.entry("undeclared")
        with self.assertRaises(ValueError):
            probes.arguments("sample", dict(b=0))
        duplicate = copy.deepcopy(catalog)
        duplicate["entries"].append(declaration)
        with self.assertRaises(ValueError):
            ProbeCatalog(duplicate, lambda name: dict(value=32))
        with self.assertRaises(ValueError):
            ProbeCatalog(dict(catalog, schema=2), lambda name: dict(value=32))

    def test_catalog_prepares_buffers_without_inventing_private_layouts(self):
        def catalog(kind):
            return ProbeCatalog(dict(schema=1, image="fixture", entries=[dict(
                symbol="sample", adapter="body", signature="example",
                arguments=[dict(name="output", rust_type=kind)])]),
                lambda name: dict(value=32))
        probes = catalog("&mut [[u16; 4]; 3]")
        phase = probes.invoke("sample", dict(output=Buffer(0x1000, [1, 2], lifetime="session")))
        self.assertEqual(phase["arguments"], [0x1000])
        self.assertEqual(phase["memory"], [region(0x1000, 24, [1, 2], lifetime="session")])
        entered = single_argument_entry(phase, 64)
        self.assertEqual(entered["entry"], 64)
        self.assertEqual(entered["arguments"], [32, 0x1000])
        self.assertEqual(phase["entry"], 32)
        with self.assertRaises(ValueError):
            probes.invoke("sample", dict(output=Buffer(0x1001)))
        with self.assertRaises(ValueError):
            probes.invoke("sample", dict(output=Buffer(0x1000, [0]*25)))
        with self.assertRaises(ValueError):
            catalog("&mut PrivateOwner").invoke("sample", dict(output=Buffer(0x1000)))
        parameters = [dict(name=name, rust_type="&mut [u8; 8]") for name in ("left", "right")]
        paired = ProbeCatalog(dict(schema=1, image="fixture", entries=[dict(
            symbol="paired", adapter="body", signature="example", arguments=parameters)]),
            lambda name: dict(value=32))
        with self.assertRaises(ValueError):
            paired.invoke("paired", dict(left=Buffer(0x1000), right=Buffer(0x1004)))
        with self.assertRaises(ValueError):
            single_argument_entry(invocation(32, [1, 2]), 64)

    def test_authentication_precedes_capture(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source = root / "source"
            source.write_bytes(b"changed input")
            runner = Runner(root / "absent-binary", root, root / "project", "watchdog")
            with self.assertRaises(ValueError):
                runner.capture([source], ["phy"], ["wrong-hash"])
            self.assertFalse((root / "input-0").exists())
            self.assertFalse((root / "project").exists())


if __name__ == "__main__":
    unittest.main()
