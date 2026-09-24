"""Explicit preparation of native Next requests and captured probe catalogs.

This module supplies no hardware model or expected result. Callers select every
input, peripheral assumption, reset and observation relation. Replay consumes
Next's retained request and never invokes these builders again.
"""
import copy
from dataclasses import dataclass
import hashlib
import json
import re
import shutil
import subprocess
import time

TIMELINE = dict(reads=False, writes=False, atomics=False, branches=False)


class Runner:
    """Run supervised Next operations and retain their requests and diagnostics."""

    def __init__(self, binary, run, project, limit_mode):
        self.binary = binary.resolve()
        self.run = run
        self.project = project
        self.limit_mode = limit_mode

    def doc(self, name, value):
        path = self.run / (name + ".request.json")
        path.write_text(json.dumps(value))
        return str(path)

    def call(self, name, args, expected=0):
        cmd = [str(self.binary), "--format", "json", args[0], "--project", str(self.project)]
        if args[0] != "init":
            cmd += ["--limit-mode", self.limit_mode, "--timeout-secs", "600",
                    "--working-memory-mib", "256", "--max-work-units", "2000000000"]
        cmd += list(map(str, args[1:]))
        start = time.monotonic()
        result = subprocess.run(cmd, capture_output=True)
        (self.run / (name + ".json")).write_bytes(result.stdout)
        (self.run / (name + ".stderr")).write_bytes(result.stderr)
        print(name, result.returncode, round(time.monotonic() - start, 2), flush=True)
        if result.returncode != expected:
            raise RuntimeError(f"{name}: exit {result.returncode}, expected {expected}: "
                               + result.stderr.decode()[-2500:])
        return json.loads(result.stdout) if result.stdout else None

    def capture(self, sources, roles, expected_hashes):
        """Authenticate selected inputs, import copies, then remove those copies.

        None in expected_hashes explicitly selects digest identification without
        a pinned expected value, as required for freshly compiled production.
        """
        if not (len(sources) == len(roles) == len(expected_hashes)):
            raise ValueError("capture roles, inputs and expectations must correspond")
        identities = [hashlib.sha256(path.read_bytes()).hexdigest() for path in sources]
        for role, actual, expected in zip(roles, identities, expected_hashes):
            if expected is not None and actual != expected:
                raise ValueError(f"{role}: authenticated input mismatch: {actual}")
        local = [self.run / f"input-{i}" for i in range(len(sources))]
        for source, destination in zip(sources, local):
            shutil.copyfile(source, destination)
        self.call("init", ["init"])
        arguments = [item for role, path in zip(roles, local)
                     for item in ("--input", role + "=" + str(path))]
        revision = self.call("import", ["import"] + arguments)["run"]["revision"]
        for path in local:
            path.unlink()
        return revision, identities


def symbol(inventory, input_index, name):
    """Resolve exactly one defined symbol in a caller-selected captured input."""
    matches = [s for o in inventory[input_index]["inventory"]["objects"] if o["elf"]
               for s in o["elf"]["symbols"]
               if bytes(s["name"]).decode() == name and s["raw_section"]]
    if len(matches) != 1:
        raise ValueError(f"missing or ambiguous symbol {name}: {len(matches)} matches")
    return matches[0]


@dataclass(frozen=True)
class Buffer:
    """Explicit placement and byte seed; extent comes from a fixed-array ABI type."""
    address: int
    data: tuple = ()
    fill: int | None = None
    lifetime: str = "phase"


def array_layout(rust_type):
    spelling = rust_type.replace(" ", "")
    if not spelling.startswith("&"):
        raise ValueError("automatic buffers require a fixed-array reference")
    spelling = spelling.removeprefix("&").removeprefix("mut")

    def layout(value):
        array = re.fullmatch(r"\[(.*);([0-9]+)\]", value)
        if array:
            size, alignment = layout(array[1])
            return size * int(array[2]), alignment
        scalar = re.fullmatch(r"[iu](8|16|32)", value)
        if scalar:
            size = int(scalar[1]) // 8
            return size, size
        raise ValueError(f"explicit buffer layout required for {rust_type}")

    if not spelling.startswith("["):
        raise ValueError("automatic buffers require a fixed-array reference")
    return layout(spelling)


class ProbeCatalog:
    """Resolve declared entries from the same captured ELF as their metadata."""

    def __init__(self, catalog, resolve):
        if (catalog.get("schema") != 1 or not isinstance(catalog.get("image"), str)
                or not catalog["image"] or not catalog.get("entries")):
            raise ValueError("unsupported or empty probe catalog")
        self.entries = {}
        self.resolve = resolve
        for entry in catalog["entries"]:
            name = entry["symbol"]
            if (not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name)
                    or name in self.entries or entry["adapter"] not in ("call", "body")):
                raise ValueError(f"invalid or duplicate probe {name}")
            # Resolve all declarations, not only entries selected by this scenario.
            resolve(name)
            self.entries[name] = entry

    @classmethod
    def capture(cls, runner, revision, inventory, input_index):
        matches = [(obj, section)
                   for obj in inventory[input_index]["inventory"]["objects"] if obj["elf"]
                   for section in obj["elf"]["sections"]
                   if bytes(section["name"]) == b".blobray.probes"]
        if len(matches) != 1:
            raise ValueError("expected one captured probe catalog section")
        obj, section = matches[0]
        request = dict(occurrence=dict(revision=revision,
                       source=dict(kind="input", input=input_index),
                       object=obj["id"], symbol=None), analyses=[],
                       ranges=[dict(kind="section", section=section["index"],
                                    offset=0, length=section["size"])])
        output = runner.run / "probe-catalog"
        runner.call("probe-catalog", ["data", "--request", runner.doc("probe-catalog", request),
                                      "--output", output])
        def resolve(name):
            selected = symbol(inventory, input_index, name)
            index = selected.get("extended_section") or selected["raw_section"]
            sections = [s for s in obj["elf"]["sections"] if s["index"] == index]
            if (selected["id"]["object"] != obj["id"] or selected["symbol_type"] != 2
                    or len(sections) != 1):
                raise ValueError(f"probe is not a function in the catalog image: {name}")
            section = sections[0]
            if (section["flags"] & 6 != 6 or not section["address"] <= selected["value"]
                    < section["address"] + section["size"]):
                raise ValueError(f"probe is not in allocated executable code: {name}")
            return selected

        return cls(json.loads((output / "data.bin").read_bytes()), resolve)

    def entry(self, name):
        if name not in self.entries:
            raise ValueError(f"undeclared probe {name}")
        return self.resolve(name)

    def invoke(self, name, values, *, models=(), observe=()):
        """Prepare named arguments and fixed-array buffers from the declaration.

        Buffer addresses, byte contents and lifetime are explicit; only size and
        alignment follow the declared primitive array. Opaque layouts are never
        guessed. Use a raw guest address to refer to already-owned memory.
        """
        parameters = self.entries[name]["arguments"]
        values = dict(values)
        memory = []
        for parameter in parameters:
            value = values.get(parameter["name"])
            if isinstance(value, Buffer):
                length, alignment = array_layout(parameter["rust_type"])
                if value.address % alignment:
                    raise ValueError("misaligned probe buffer")
                new = region(value.address, length, value.data, value.fill, value.lifetime)
                for old in memory:
                    seed = old["seed"]
                    if value.address < seed["address"] + seed["length"] and seed["address"] < value.address + length:
                        raise ValueError("overlapping probe buffer owners")
                memory.append(new)
                values[parameter["name"]] = value.address
        return invocation(self.entry(name)["value"], self.arguments(name, values),
                          memory, models, observe)

    def arguments(self, name, values):
        """Lower named ABI scalars/pointers with explicit signed range checks.

        References are guest addresses, never host references or inferred private
        layouts. Unknown words remain None. Complex layouts require a caller's
        explicit adapter instead of an inferred representation.
        """
        parameters = self.entries[name]["arguments"]
        if set(values) != {p["name"] for p in parameters}:
            raise ValueError(f"argument names do not match {name}")
        return [abi_word(p["rust_type"], values[p["name"]]) for p in parameters]


def abi_word(rust_type, value):
    if value is None:
        return None
    if not isinstance(value, int):
        raise ValueError("ABI word must be an integer or explicit unknown")
    spelling = rust_type.replace(" ", "")
    if spelling == "bool":
        if value not in (0, 1):
            raise ValueError("Rust bool requires zero or one")
        return int(value)
    if spelling.startswith(("&", "*const", "*mut")):
        bits, signed = 32, False
    elif re.fullmatch(r"[iu](8|16|32)", spelling):
        bits, signed = int(spelling[1:]), spelling[0] == "i"
    elif spelling in ("usize", "isize"):
        bits, signed = 32, spelling == "isize"
    else:
        raise ValueError(f"explicit ABI adapter required for {rust_type}")
    lower = -(1 << (bits - 1)) if signed else 0
    upper = (1 << (bits - (1 if signed else 0))) - 1
    if not lower <= value <= upper:
        raise ValueError(f"value outside {rust_type}: {value}")
    return value & 0xffffffff


def words(values, *, pad_to=None, fill=None):
    """Encode RV32 words; padding exists only when both count and fill are explicit."""
    values = list(values)
    if pad_to is not None:
        if fill is None or len(values) > pad_to:
            raise ValueError("word padding requires explicit fill and sufficient capacity")
        values += [fill] * (pad_to - len(values))
    elif fill is not None:
        raise ValueError("fill requires pad_to")
    return [b for value in values for b in abi_word("u32", value).to_bytes(4, "little")]


def seed(address, length, data=(), fill=None):
    data = list(data)
    if address < 0 or length < 0 or address + length > 1 << 32 or len(data) > length:
        raise ValueError("invalid RV32 memory extent")
    if any(not isinstance(byte, int) or not 0 <= byte <= 255 for byte in data):
        raise ValueError("seed bytes must be octets")
    if fill is not None and not 0 <= fill <= 255:
        raise ValueError("fill must be an octet")
    return dict(address=address, length=length, fill=fill, bytes=data)


def region(address, length, data=(), fill=None, lifetime="phase"):
    return dict(seed=seed(address, length, data, fill), lifetime=lifetime)


def invocation(entry, arguments=(), memory=(), models=(), observe=()):
    return dict(entry=entry, goal={"kind": "return"}, arguments=list(arguments),
                memory=copy.deepcopy(list(memory)), models=copy.deepcopy(list(models)),
                calls=[], tables=[], services=[], observe_memory=copy.deepcopy(list(observe)),
                observe_calls=None, observe_timeline=TIMELINE.copy())


def selection(address, length):
    return dict(name="selected-output", address=address, length=length)


def relation(memory=False):
    return dict(effects=None, projection=None, returns=dict(low=False, high=False),
                events=dict(timeline=TIMELINE.copy(), mmio_read=True, mmio_write=True,
                            fence=True, delay=True),
                memory=[dict(vendor=0, replacement=0)] if memory else [],
                calls=False, reviewed_calls=None)


def case(name, vendor, replacement, reset="cold", memory=False):
    return dict(name=name, reset=reset, relation=relation(memory),
                vendor=vendor, replacement=replacement)


def compare(call, doc, label, vendor, replacement, cases, verdict, maximum):
    request = dict(schema=17, vendor=vendor, replacement=replacement, binding="shared-core",
                   cases=cases, max_events=maximum)
    identity = call(label, ["compare", "--request", doc(label, request)])["run"]["execution"]
    evidence = call(label + "-evidence", ["execution", "--id", identity])
    if evidence["summary"]["manifest"]["verdict"] != verdict:
        raise AssertionError(f"{label}: expected {verdict}")
    return identity, evidence


def single_argument_entry(phase, trampoline):
    """Enter a prepared one-argument probe through an explicitly selected shim."""
    if len(phase["arguments"]) != 1:
        raise ValueError("single-argument trampoline requires exactly one argument")
    phase = copy.deepcopy(phase)
    phase["arguments"] = [phase["entry"], phase["arguments"][0]]
    phase["entry"] = trampoline
    return phase
