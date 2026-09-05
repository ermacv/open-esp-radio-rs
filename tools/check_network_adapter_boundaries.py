#!/usr/bin/env python3
"""Validate resolved network ownership boundaries, independently of Rust spellings."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib


class BoundaryError(Exception):
    """Cargo metadata is incomplete or a resolved dependency crosses a boundary."""


def require(condition, message):
    if not condition:
        raise BoundaryError(message)


def registry(package):
    return (package["source"] or "").startswith("registry+")


class Graph:
    """A validated Cargo resolve graph indexed by package identity, never aliases."""

    def __init__(self, document):
        require(isinstance(document, dict), "metadata must be an object")
        require(isinstance(document.get("packages"), list), "metadata packages missing")
        self.packages = {}
        for package in document["packages"]:
            require(isinstance(package, dict), "invalid package record")
            for key in ("id", "name", "version", "manifest_path"):
                require(isinstance(package.get(key), str) and package[key], f"package {key} missing")
            require("source" in package and (package["source"] is None or isinstance(package["source"], str)),
                    "package source missing or invalid")
            require(Path(package["manifest_path"]).is_absolute(), "package manifest path must be absolute")
            require(package["id"] not in self.packages, "duplicate package identity")
            require(isinstance(package.get("features"), dict), "declared feature map missing")
            require(isinstance(package.get("dependencies"), list), "declared dependencies missing")
            for dependency in package["dependencies"]:
                require(isinstance(dependency, dict), "invalid declared dependency")
                require(isinstance(dependency.get("name"), str) and dependency["name"], "declared package name missing")
                require("kind" in dependency and dependency["kind"] in (None, "build", "dev"),
                        "declared dependency kind missing or unknown")
                require("source" in dependency and (dependency["source"] is None or isinstance(dependency["source"], str)),
                        "declared dependency source missing or invalid")
                require(isinstance(dependency.get("optional"), bool), "declared optional dependency status missing")
                require(dependency.get("path") is None or
                        (isinstance(dependency["path"], str) and Path(dependency["path"]).is_absolute()),
                        "declared dependency path invalid")
            self.packages[package["id"]] = package
        resolve = document.get("resolve")
        require(isinstance(resolve, dict) and isinstance(resolve.get("nodes"), list), "resolved nodes missing")
        self.nodes = {}
        for node in resolve["nodes"]:
            require(isinstance(node, dict), "invalid resolve node")
            require(isinstance(node.get("id"), str) and node["id"] in self.packages, "unknown resolve identity")
            require(node["id"] not in self.nodes, "duplicate resolve identity")
            require(isinstance(node.get("features"), list) and all(isinstance(f, str) for f in node["features"]),
                    "resolved features missing or invalid")
            require(isinstance(node.get("deps"), list), "resolved dependency edges missing")
            for dependency in node["deps"]:
                require(isinstance(dependency, dict), "invalid dependency edge")
                require(isinstance(dependency.get("name"), str) and dependency["name"], "dependency alias missing")
                require(isinstance(dependency.get("pkg"), str) and dependency["pkg"] in self.packages,
                        "dependency package identity missing")
                kinds = dependency.get("dep_kinds")
                require(isinstance(kinds, list) and kinds, "dependency kinds missing")
                for kind in kinds:
                    require(isinstance(kind, dict) and "kind" in kind and kind["kind"] in (None, "build", "dev"),
                            "dependency kind missing or unknown")
                    require("target" in kind and (kind["target"] is None or isinstance(kind["target"], str)),
                            "dependency target missing or invalid")
            self.nodes[node["id"]] = node
        for node in self.nodes.values():
            for dependency in node["deps"]:
                require(dependency["pkg"] in self.nodes, "dependency resolve node missing")

    def root(self, manifest):
        matches = [p["id"] for p in self.packages.values() if Path(p["manifest_path"]) == manifest.resolve()]
        require(len(matches) == 1, f"manifest must identify exactly one package: {manifest}")
        require(matches[0] in self.nodes, "root resolve node missing")
        return matches[0]

    def reachable(self, root):
        """Return production/build paths; dev-only edges never confer ownership."""
        paths = {root: [root]}
        pending = [root]
        while pending:
            current = pending.pop()
            for edge in self.nodes[current]["deps"]:
                if all(kind["kind"] == "dev" for kind in edge["dep_kinds"]):
                    continue
                child = edge["pkg"]
                if child not in paths:
                    paths[child] = paths[current] + [child]
                    pending.append(child)
        return paths


NETWORK = "open-esp-radio-network"
OWNED = "open-esp-radio-embassy-net"
COMPAT = "open-esp-radio-embassy-net-compat"
BRIDGE = "open-esp-radio-esp32s31-wifi-embassy-compat"


def optimized(package):
    return (package["name"] in (OWNED, "xarxa", "xarxa-driver")
            or (package["name"] in ("embassy-net", "embassy-net-driver") and not registry(package)))


def executor_or_stack(package):
    name = package["name"]
    return name.startswith(("embassy-", "open-esp-radio-embassy")) or name in ("xarxa", "xarxa-driver")


def physical(package, repository):
    path = Path(package["manifest_path"])
    return (package["name"].startswith("open-esp-radio-esp32s31")
            or path.is_relative_to(repository / "driver/chips"))


def audit(graph, manifest, boundary, repository):
    root = graph.root(manifest)
    # These three leaves also forbid declarations that are currently disabled
    # optional features. Traversal alone cannot enforce that future opt-in boundary.
    for dependency in graph.packages[root]["dependencies"]:
        if dependency["kind"] == "dev":
            continue
        name = dependency["name"]
        path = Path(dependency["path"]) if dependency.get("path") else None
        forbidden = boundary == "neutral"
        if boundary == "compat":
            forbidden = not registry(dependency) and name != NETWORK
        if boundary == "owned":
            forbidden = (name.startswith("open-esp-radio-esp32s31") or name == "open-esp-radio-dma"
                         or (name == "embassy-net-driver" and registry(dependency))
                         or (path is not None and any(path.is_relative_to(repository / directory)
                                                    for directory in ("driver/chips", "driver/memory"))))
        require(not forbidden, f"{boundary}: forbidden declared production dependency: {name}")
    paths = graph.reachable(root)
    dependencies = [graph.packages[identity] for identity in paths if identity != root]

    def reject(predicate, reason):
        for package in dependencies:
            if predicate(package):
                chain = " -> ".join(graph.packages[p]["name"] for p in paths[package["id"]])
                raise BoundaryError(f"{boundary}: {reason}: {chain} ({package['source'] or package['manifest_path']})")

    def required(predicate, description):
        require(any(predicate(p) for p in dependencies), f"{boundary}: missing {description}")

    def released_driver():
        reject(lambda p: p["name"] == "embassy-net-driver" and
               (not registry(p) or p["version"] != "0.2.0"), "requires released embassy-net-driver 0.2.0")
        required(lambda p: p["name"] == "embassy-net-driver" and registry(p) and p["version"] == "0.2.0",
                 "released embassy-net-driver 0.2.0")

    if boundary == "neutral":
        reject(lambda _: True, "neutral network values acquired a production dependency")
    elif boundary == "compat":
        reject(lambda p: not registry(p) and p["name"] != NETWORK, "compatibility adapter acquired a non-neutral dependency")
        released_driver()
    elif boundary == "owned":
        reject(lambda p: p["name"] == "embassy-net-driver" and registry(p), "owned adapter acquired the released driver contract")
        reject(lambda p: physical(p, repository), "owned adapter acquired physical radio ownership")
        # The portable datapath already depends on stable-memory contracts;
        # those transitive contracts do not confer physical radio ownership.
        reject(lambda p: len(paths[p["id"]]) == 2 and
               (p["name"] == "open-esp-radio-dma" or
                Path(p["manifest_path"]).is_relative_to(repository / "driver/memory")),
               "owned adapter acquired a direct physical-memory dependency")
    elif boundary in ("research", "datapath"):
        reject(executor_or_stack, "portable contract acquired an executor or network stack")
        if boundary == "datapath":
            reject(lambda p: p["name"].startswith("open-esp-radio-esp32s31") or
                   Path(p["manifest_path"]).is_relative_to(repository / "driver/chips"),
                   "radio-native datapath acquired a chip dependency")
    elif boundary in ("radio-core", "compat-bridge", "compat-product"):
        reject(optimized, "compatibility/radio core acquired the optimized network graph")
        if boundary != "radio-core":
            released_driver()
        if boundary == "compat-product":
            reject(lambda p: p["name"] in ("embassy-net", "embassy-net-driver") and not registry(p),
                   "compatibility product acquired a non-release Embassy network package")
            required(lambda p: p["name"] == COMPAT, "compatibility network adapter")
            required(lambda p: p["name"] == BRIDGE, "compatibility radio bridge")
    elif boundary == "owned-product":
        # esp-hal may independently use the released driver trait. The actual
        # network leaves must remain exclusive, not every incidental trait use.
        reject(lambda p: p["name"] in (COMPAT, BRIDGE),
               "owned product acquired a compatibility network leaf")
        required(lambda p: p["name"] == OWNED, "owned network adapter")
        required(lambda p: p["name"] == "embassy-net-driver" and (p["source"] or "").startswith("git+"),
                 "owned Embassy driver contract")
    else:
        raise BoundaryError(f"unknown boundary: {boundary}")

    if boundary in ("owned-product", "compat-product"):
        expected = "owned-network" if boundary == "owned-product" else "compat-network"
        other = "compat-network" if expected == "owned-network" else "owned-network"
        features = graph.nodes[root]["features"]
        require(expected in features and other not in features, f"{boundary}: selected feature boundary is not exclusive")
    if boundary == "radio-core":
        require("owned-network" not in graph.nodes[root]["features"], "radio-core: owned-network unexpectedly enabled")


def profiles():
    product = "driver/integration/esp32s31/embassy/ieee80211/Cargo.toml"
    return [
        ("neutral", "driver/network/interface/Cargo.toml", []),
        ("compat", "driver/network/adapters/embassy/compat/Cargo.toml", []),
        ("owned", "driver/network/adapters/embassy/owned/Cargo.toml", []),
        ("research", "driver/network/research/Cargo.toml", []),
        ("datapath", "driver/ieee80211/datapath/Cargo.toml", []),
        ("radio-core", "driver/runtime/embassy/esp32s31/ieee80211/Cargo.toml", ["--no-default-features"]),
        ("compat-bridge", "driver/adapters/embassy/esp32s31/ieee80211-compat/Cargo.toml", []),
        ("owned-product", product, []),
        ("compat-product", product, ["--no-default-features", "--features", "compat-network"]),
    ]


def toml_value(value):
    if isinstance(value, str):
        return json.dumps(value)
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    if isinstance(value, dict):
        return "{ " + ", ".join(json.dumps(key) + " = " + toml_value(item)
                               for key, item in value.items()) + " }"
    raise BoundaryError(f"unsupported Cargo patch value: {value!r}")


def load_graph(manifest, features, target=None):
    """Resolve one real consumer, retaining origin lock pins and workspace patches.

    Cargo metadata otherwise unifies features across every workspace member,
    including unrelated consumers. Only the temporary lockfile may be updated
    to insert the probe; every resolved dependency must still use an origin pin.
    """
    located = subprocess.run(["cargo", "locate-project", "--workspace", "--message-format", "json",
                              "--manifest-path", str(manifest)], cwd=manifest.parent, check=True, text=True, stdout=subprocess.PIPE)
    location = json.loads(located.stdout)
    require(isinstance(location, dict) and isinstance(location.get("root"), str), "workspace root missing")
    origin = Path(location["root"])
    require(origin.is_absolute(), "workspace manifest path must be absolute")
    origin_toml = tomllib.loads(origin.read_text())
    package = tomllib.loads(manifest.read_text()).get("package", {})
    require(isinstance(package, dict) and isinstance(package.get("name"), str), "audited package name missing")
    lock_path = origin.with_name("Cargo.lock")
    lock_text = lock_path.read_text()
    lock = tomllib.loads(lock_text)
    require(isinstance(lock.get("package"), list), "origin lock package catalog missing")
    pins = set()
    for entry in lock["package"]:
        require(isinstance(entry, dict) and isinstance(entry.get("name"), str)
                and isinstance(entry.get("version"), str), "invalid origin lock package")
        pins.add((entry["name"], entry["version"], entry.get("source")))
    selected = []
    if "--features" in features:
        selected = features[features.index("--features") + 1].split(",")
    dependency = {"package": package["name"], "path": str(manifest.parent),
                  "default-features": "--no-default-features" not in features, "features": selected}
    probe = "oer-network-boundary-probe"
    require(not any(pin[0] == probe for pin in pins), "probe package conflicts with origin lock")
    text = ('[package]\nname = "' + probe + '"\nversion = "0.0.0"\nedition = "2024"\n'
            '[workspace]\nresolver = "3"\n[dependencies]\naudited = ' + toml_value(dependency) + "\n")
    def overrides(patches):
        result = ""
        for source, entries in patches.items():
            require(isinstance(entries, dict), "invalid workspace patch table")
            result += "[patch." + json.dumps(source) + "]\n"
            for name, specification in entries.items():
                require(isinstance(specification, dict), "invalid workspace patch specification")
                specification = dict(specification)
                if "path" in specification:
                    specification["path"] = str((origin.parent / specification["path"]).resolve())
                result += json.dumps(name) + " = " + toml_value(specification) + "\n"
        replacements = origin_toml.get("replace", {})
        if replacements:
            result += "[replace]\n"
            for name, specification in replacements.items():
                specification = dict(specification)
                if "path" in specification:
                    specification["path"] = str((origin.parent / specification["path"]).resolve())
                result += json.dumps(name) + " = " + toml_value(specification) + "\n"
        return result

    patches = origin_toml.get("patch", {})
    with tempfile.TemporaryDirectory(prefix="oer-network-metadata-") as temporary:
        scratch = Path(temporary)
        scratch_manifest = scratch / "Cargo.toml"
        scratch_manifest.write_text(text + overrides(patches))
        (scratch / "src").mkdir()
        (scratch / "src/lib.rs").write_text("")
        (scratch / "Cargo.lock").write_text(lock_text)
        command = ["cargo", "metadata", "--format-version", "1", "--manifest-path", str(scratch_manifest)]
        if target:
            command += ["--filter-platform", target]
        def resolve(mode):
            output = subprocess.run([*command, mode], cwd=origin.parent, check=True, text=True,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            graph = Graph(json.loads(output.stdout))
            probe_id = graph.root(scratch_manifest)
            for identity, resolved in graph.packages.items():
                if identity != probe_id:
                    pin = (resolved["name"], resolved["version"], resolved["source"])
                    require(pin in pins, f"isolated dependency drifted from origin lock: {pin}")
            return graph

        # Only this copied lock is initialized; every real package stays pinned.
        graph = resolve("--offline")
        unused = tomllib.loads((scratch / "Cargo.lock").read_text()).get("patch", {}).get("unused", [])
        if unused:
            # Cargo can nondeterministically reorder unused override records,
            # making --locked reject an unchanged graph. Remove only overrides
            # Cargo proved unused, then verify the resolved graph is identical.
            unused_names = {entry["name"] for entry in unused}
            declarations = [(source, name) for source, entries in patches.items()
                            for name, spec in entries.items()
                            if spec.get("package", name) in unused_names]
            for name in unused_names:
                matches = [(source, key) for source, key in declarations
                           if patches[source][key].get("package", key) == name]
                require(len(matches) == 1, f"cannot uniquely identify unused patch: {name}")
            retained = {source: {name: spec for name, spec in entries.items()
                                 if (source, name) not in declarations}
                        for source, entries in patches.items()}
            scratch_manifest.write_text(text + overrides(retained))
            pruned = resolve("--offline")
            require(pruned.nodes == graph.nodes, "removing an unused patch changed resolved dependencies")
            graph = pruned
        locked = resolve("--locked")
        require(locked.nodes == graph.nodes, "locked dependency resolution changed after initialization")
        graph = locked
        graph.root(manifest)
        return graph


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check-dependencies-only", action="store_true", help="skip the standalone compile checks")
    args = parser.parse_args()
    repository = Path(__file__).resolve().parent.parent
    target = "riscv32imafc-unknown-none-elf"
    try:
        for boundary, relative_manifest, features in profiles():
            manifest = repository / relative_manifest
            product = boundary.endswith("-product")
            graph = load_graph(manifest, features, target if product else None)
            audit(graph, manifest, boundary, repository)
            print(f"network boundary passed: {boundary}", flush=True)
        if not args.check_dependencies_only:
            # Keep the original host all-target profiles and both concrete product leaves.
            checks = [(manifest, features, boundary.endswith("-product"))
                      for boundary, manifest, features in profiles() if boundary != "neutral"]
            checks.append(("driver/runtime/embassy/esp32s31/ieee80211/Cargo.toml", [], False))
            for manifest, features, product in checks:
                command = ["cargo", "check", "--manifest-path", manifest, "--locked", *features]
                command += ["--target", target] if product else ["--all-targets"]
                subprocess.run(command, cwd=repository, check=True)
        print("network adapter dependency boundaries are clean" if args.check_dependencies_only else
              "network adapter compile and dependency boundaries are clean")
    except (BoundaryError, json.JSONDecodeError, tomllib.TOMLDecodeError, OSError, subprocess.CalledProcessError) as error:
        print(f"network boundary audit failed: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr.strip(), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
