"""Exercise boundary decisions on actual Cargo graphs and damaged resolve data."""

import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


HELPER = Path(__file__).resolve().parents[1] / "check_network_adapter_boundaries.py"
SPEC = importlib.util.spec_from_file_location("network_boundaries", HELPER)
BOUNDARIES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BOUNDARIES)


class NetworkBoundaryTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="oer-network-boundary-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.write("Cargo.toml", '[workspace]\nmembers = ["adapter", "helper", "driver/chips/test-radio"]\nresolver = "3"\n')
        self.package("driver/chips/test-radio", "device-registers")
        self.package("helper", "packet-helper")
        self.package("adapter", "network-adapter-fixture")
        self.manifest = self.root / "adapter/Cargo.toml"

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def package(self, directory, name, dependencies=""):
        self.write(f"{directory}/Cargo.toml", f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2024"\n{dependencies}')
        self.write(f"{directory}/src/lib.rs", "")

    def metadata(self):
        environment = {**os.environ, "CARGO_NET_OFFLINE": "true"}
        subprocess.run(["cargo", "generate-lockfile"], cwd=self.root, env=environment,
                       check=True, capture_output=True, text=True)
        result = subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked",
                                 "--manifest-path", str(self.manifest)], cwd=self.root,
                                env=environment, check=True, capture_output=True, text=True)
        return json.loads(result.stdout)

    def audit(self, document, boundary="owned"):
        BOUNDARIES.audit(BOUNDARIES.Graph(document), self.manifest, boundary, self.root)

    def test_renamed_transitive_build_dependency_cannot_hide_chip_ownership(self):
        self.package("adapter", "network-adapter-fixture",
                     '[dependencies]\nqueue = { package = "packet-helper", path = "../helper" }\n')
        self.package("helper", "packet-helper",
                     '[build-dependencies]\ngenerator = { package = "device-registers", path = "../driver/chips/test-radio" }\n')
        with self.assertRaisesRegex(BOUNDARIES.BoundaryError,
                                    "network-adapter-fixture -> packet-helper -> device-registers"):
            self.audit(self.metadata())

    def test_dev_only_dependency_does_not_confer_production_ownership(self):
        self.package("adapter", "network-adapter-fixture",
                     '[dev-dependencies]\nfixture = { package = "device-registers", path = "../driver/chips/test-radio" }\n')
        document = self.metadata()
        self.assertTrue(any(p["name"] == "device-registers" for p in document["packages"]))
        self.audit(document)

    def test_normal_edge_remains_forbidden_when_the_same_package_is_also_a_dev_dependency(self):
        dependency = 'fixture = { package = "device-registers", path = "../driver/chips/test-radio" }\n'
        self.package("adapter", "network-adapter-fixture",
                     '[dependencies]\n' + dependency + '[dev-dependencies]\n' + dependency)
        with self.assertRaisesRegex(BOUNDARIES.BoundaryError, "device-registers"):
            self.audit(self.metadata())

    def test_unreachable_workspace_package_is_not_a_production_dependency(self):
        document = self.metadata()
        self.assertTrue(any(p["name"] == "device-registers" for p in document["packages"]))
        self.audit(document)

    def test_missing_or_malformed_resolve_information_fails_closed(self):
        valid = self.metadata()
        root_id = next(p["id"] for p in valid["packages"] if p["manifest_path"] == str(self.manifest))
        mutations = [
            lambda d: d.pop("resolve"),
            lambda d: d.update(resolve=None),
            lambda d: d["resolve"].update(nodes=[]),
            lambda d: d["packages"].append(d["packages"][0]),
            lambda d: d["resolve"]["nodes"].append(d["resolve"]["nodes"][0]),
            lambda d: next(n for n in d["resolve"]["nodes"] if n["id"] == root_id).pop("deps"),
            lambda d: next(n for n in d["resolve"]["nodes"] if n["id"] == root_id).pop("features"),
        ]
        for mutate in mutations:
            with self.subTest(mutation=mutate):
                damaged = copy.deepcopy(valid)
                mutate(damaged)
                with self.assertRaises(BOUNDARIES.BoundaryError):
                    self.audit(damaged)

    def test_missing_edge_kinds_or_destination_cannot_erase_a_forbidden_dependency(self):
        self.package("adapter", "network-adapter-fixture",
                     '[build-dependencies]\nfixture = { package = "device-registers", path = "../driver/chips/test-radio" }\n')
        valid = self.metadata()
        root_id = next(p["id"] for p in valid["packages"] if p["manifest_path"] == str(self.manifest))
        mutations = [
            lambda edge: edge.pop("dep_kinds"),
            lambda edge: edge.update(dep_kinds=[]),
            lambda edge: edge.update(dep_kinds=[{"kind": "unknown", "target": None}]),
            lambda edge: edge.update(pkg="absent-package-identity"),
        ]
        for mutate in mutations:
            with self.subTest(mutation=mutate):
                damaged = copy.deepcopy(valid)
                mutate(next(n for n in damaged["resolve"]["nodes"] if n["id"] == root_id)["deps"][0])
                with self.assertRaises(BOUNDARIES.BoundaryError):
                    self.audit(damaged)

    def test_cyclic_graph_terminates_and_still_rejects_reachable_chip(self):
        self.package("adapter", "network-adapter-fixture",
                     '[dependencies]\nhelper = { package = "packet-helper", path = "../helper" }\n')
        self.package("helper", "packet-helper",
                     '[build-dependencies]\nfixture = { package = "device-registers", path = "../driver/chips/test-radio" }\n')
        document = self.metadata()
        packages = {p["name"]: p["id"] for p in document["packages"]}
        chip = next(n for n in document["resolve"]["nodes"] if n["id"] == packages["device-registers"])
        chip["deps"].append({"name": "loop", "pkg": packages["network-adapter-fixture"],
                             "dep_kinds": [{"kind": None, "target": None}]})
        with self.assertRaisesRegex(BOUNDARIES.BoundaryError, "physical radio ownership"):
            self.audit(document)

    def test_disabled_optional_dependency_still_violates_a_declared_leaf_boundary(self):
        self.package("adapter", "network-adapter-fixture",
                     '[dependencies]\nfixture = { package = "device-registers", path = "../driver/chips/test-radio", optional = true }\n')
        document = self.metadata()
        graph = BOUNDARIES.Graph(document)
        self.assertEqual(len(graph.reachable(graph.root(self.manifest))), 1)
        for boundary in ("neutral", "compat", "owned"):
            with self.subTest(boundary=boundary):
                with self.assertRaisesRegex(BOUNDARIES.BoundaryError, "forbidden declared production dependency"):
                    self.audit(document, boundary)

    def test_unrelated_member_feature_does_not_activate_a_dependency_for_the_audited_consumer(self):
        self.write("Cargo.toml", '[workspace]\nmembers = ["adapter", "helper", "consumer", "driver/chips/test-radio"]\nresolver = "3"\n')
        self.package("adapter", "network-adapter-fixture",
                     '[dependencies]\nhelper = { package = "packet-helper", path = "../helper", default-features = false }\n'
                     '[features]\ndefault = ["plain"]\nplain = []\n')
        self.package("helper", "packet-helper",
                     '[dependencies]\nchip = { package = "device-registers", path = "../driver/chips/test-radio", optional = true }\n'
                     '[features]\nhardware = ["dep:chip"]\n')
        self.package("consumer", "unrelated-consumer",
                     '[dependencies]\nhelper = { package = "packet-helper", path = "../helper", features = ["hardware"] }\n')
        self.metadata()
        before = (self.root / "Cargo.lock").read_bytes()
        graph = BOUNDARIES.load_graph(self.manifest, [])
        root = graph.root(self.manifest)
        self.assertIn("plain", graph.nodes[root]["features"])
        BOUNDARIES.audit(graph, self.manifest, "owned", self.root)
        self.assertEqual(before, (self.root / "Cargo.lock").read_bytes())
        # The same capability must become forbidden when this consumer selects it.
        self.package("adapter", "network-adapter-fixture",
                     '[dependencies]\nhelper = { package = "packet-helper", path = "../helper", features = ["hardware"] }\n')
        graph = BOUNDARIES.load_graph(self.manifest, [])
        with self.assertRaisesRegex(BOUNDARIES.BoundaryError, "physical radio ownership"):
            BOUNDARIES.audit(graph, self.manifest, "owned", self.root)

    def test_relative_workspace_patch_preserves_the_actual_chip_dependency(self):
        self.package("adapter", "network-adapter-fixture",
                     '[dependencies]\napi = { package = "device-registers", version = "0.1" }\n')
        with (self.root / "Cargo.toml").open("a") as manifest:
            manifest.write('[patch.crates-io]\ndevice-registers = { path = "driver/chips/test-radio" }\n')
        self.metadata()
        graph = BOUNDARIES.load_graph(self.manifest, [])
        with self.assertRaisesRegex(BOUNDARIES.BoundaryError, "physical radio ownership"):
            BOUNDARIES.audit(graph, self.manifest, "owned", self.root)

    def test_unused_patch_is_removed_only_from_the_temporary_consumer(self):
        self.package("unused-override", "unused-override")
        with (self.root / "Cargo.toml").open("a") as manifest:
            manifest.write('[patch."https://example.invalid/unused"]\nunused-override = { path = "unused-override" }\n')
        self.metadata()
        before_manifest = (self.root / "Cargo.toml").read_bytes()
        before_lock = (self.root / "Cargo.lock").read_bytes()
        graph = BOUNDARIES.load_graph(self.manifest, [])
        BOUNDARIES.audit(graph, self.manifest, "owned", self.root)
        self.assertEqual(before_manifest, (self.root / "Cargo.toml").read_bytes())
        self.assertEqual(before_lock, (self.root / "Cargo.lock").read_bytes())

    def test_isolated_resolution_rejects_dependency_version_drift(self):
        self.package("adapter", "network-adapter-fixture",
                     '[dependencies]\nhelper = { package = "packet-helper", path = "../helper" }\n')
        self.metadata()
        # A changed path package may make Cargo refresh the scratch lock, but
        # the boundary audit must not silently depart from the origin pins.
        self.package("helper", "packet-helper")
        path = self.root / "helper/Cargo.toml"
        path.write_text(path.read_text().replace('version = "0.1.0"', 'version = "0.2.0"'))
        before_lock = (self.root / "Cargo.lock").read_bytes()
        with self.assertRaisesRegex(BOUNDARIES.BoundaryError, "drifted from origin lock"):
            BOUNDARIES.load_graph(self.manifest, [])
        self.assertEqual(before_lock, (self.root / "Cargo.lock").read_bytes())

    def test_released_driver_identity_requires_both_registry_source_and_version(self):
        self.package("adapter", "network-adapter-fixture",
                     '[dependencies]\napi = { package = "packet-helper", path = "../helper" }\n')
        document = self.metadata()
        package = next(p for p in document["packages"] if p["name"] == "packet-helper")
        package.update(name="embassy-net-driver", version="0.2.0", source="registry+https://github.com/rust-lang/crates.io-index")
        declaration = next(p for p in document["packages"] if p["manifest_path"] == str(self.manifest))["dependencies"][0]
        declaration.update(name=package["name"], source=package["source"])
        self.audit(document, "compat")
        for source, version in [("git+https://example.invalid/driver#fork", "0.2.0"), (package["source"], "0.3.0")]:
            with self.subTest(source=source, version=version):
                changed = copy.deepcopy(document)
                next(p for p in changed["packages"] if p["name"] == "embassy-net-driver").update(source=source, version=version)
                with self.assertRaises(BOUNDARIES.BoundaryError):
                    self.audit(changed, "compat")

    def test_dependency_check_failure_does_not_print_success(self):
        # A broken Cargo response must remain a failure at the executable boundary.
        directory = self.root / "bin"
        directory.mkdir()
        cargo = directory / "cargo"
        cargo.write_text('#!/bin/sh\nprintf "%s\\n" "{\\\"packages\\\": []}"\n')
        cargo.chmod(0o755)
        result = subprocess.run(["python3", str(HELPER), "--check-dependencies-only"],
                                env={**os.environ, "PATH": f"{directory}:{os.environ['PATH']}"},
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("workspace root missing", result.stderr)
        self.assertNotIn("boundaries are clean", result.stdout)


if __name__ == "__main__":
    unittest.main()
