#!/usr/bin/env python3
"""Validate the recovered ESP32-S31 radio SVD and generate Rust PAC identities."""

from __future__ import annotations

import argparse
import difflib
import sys
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
SVD_PATH = REPO / "svd" / "esp32s31-radio.svd"
OUTPUT_PATH = (
    REPO
    / "crates"
    / "open-esp-radio-pac-esp32s31"
    / "src"
    / "power.rs"
)


@dataclass(frozen=True)
class Field:
    name: str
    description: str
    offset: int
    width: int


@dataclass(frozen=True)
class Register:
    name: str
    description: str
    address: int
    access: str
    reset_value: int | None
    fields: tuple[Field, ...]
    dim: int
    dim_increment: int


@dataclass(frozen=True)
class Peripheral:
    name: str
    description: str
    base: int
    registers: tuple[Register, ...]


def child_text(node: ET.Element, name: str, *, required: bool = True) -> str | None:
    child = node.find(name)
    if child is None or child.text is None:
        if required:
            raise ValueError(f"missing <{name}> under <{node.tag}>")
        return None
    return " ".join(child.text.split())


def integer(node: ET.Element, name: str, *, required: bool = True) -> int | None:
    value = child_text(node, name, required=required)
    return int(value, 0) if value is not None else None


def rust_doc(description: str, indent: str = "") -> list[str]:
    words = description.split()
    lines: list[str] = []
    current = f"{indent}///"
    for word in words:
        if len(current) + len(word) + 1 > 96:
            lines.append(current)
            current = f"{indent}/// {word}"
        else:
            current += f" {word}"
    lines.append(current)
    return lines


def parse_svd() -> tuple[Peripheral, ...]:
    root = ET.parse(SVD_PATH).getroot()
    if child_text(root, "name") != "ESP32S31_RADIO":
        raise ValueError("unexpected SVD device name")
    if integer(root, "width") != 32 or integer(root, "addressUnitBits") != 8:
        raise ValueError("only a byte-addressed 32-bit device is supported")

    peripherals: list[Peripheral] = []
    occupied_addresses: dict[int, str] = {}
    for peripheral_node in root.findall("./peripherals/peripheral"):
        name = child_text(peripheral_node, "name")
        description = child_text(peripheral_node, "description")
        base = integer(peripheral_node, "baseAddress")
        registers: list[Register] = []
        for register_node in peripheral_node.findall("./registers/register"):
            register_name = child_text(register_node, "name")
            register_description = child_text(register_node, "description")
            address = base + integer(register_node, "addressOffset")
            dim = integer(register_node, "dim", required=False) or 1
            dim_increment = integer(register_node, "dimIncrement", required=False) or 0
            size = integer(register_node, "size", required=False) or 32
            access = child_text(register_node, "access", required=False) or "read-write"
            reset_value = integer(register_node, "resetValue", required=False)
            if size != 32:
                raise ValueError(f"{name}.{register_name}: size is not 32")
            if address & 3:
                raise ValueError(f"{name}.{register_name}: unaligned address {address:#x}")
            if access not in {"read-only", "write-only", "read-write"}:
                raise ValueError(f"{name}.{register_name}: unsupported access {access}")
            if dim < 1:
                raise ValueError(f"{name}.{register_name}: invalid dim {dim}")
            if dim > 1 and (dim_increment < 4 or dim_increment & 3):
                raise ValueError(
                    f"{name}.{register_name}: invalid dimIncrement "
                    f"{dim_increment:#x}"
                )
            if dim > 1 and "%s" not in register_name:
                raise ValueError(
                    f"{name}.{register_name}: dimensioned name must contain %s"
                )
            for index in range(dim):
                element_address = address + index * dim_increment
                if element_address in occupied_addresses:
                    raise ValueError(
                        f"{name}.{register_name}[{index}]: address duplicates "
                        f"{occupied_addresses[element_address]}"
                    )
                occupied_addresses[element_address] = (
                    f"{name}.{register_name}[{index}]"
                )

            fields: list[Field] = []
            used_mask = 0
            for field_node in register_node.findall("./fields/field"):
                field_name = child_text(field_node, "name")
                field_description = child_text(
                    field_node, "description", required=False
                ) or f"Field layout from {register_description}"
                offset = integer(field_node, "bitOffset")
                width = integer(field_node, "bitWidth")
                if width < 1 or width > 32 or offset < 0 or offset + width > 32:
                    raise ValueError(
                        f"{name}.{register_name}.{field_name}: invalid bit range"
                    )
                mask = ((1 << width) - 1) << offset
                if used_mask & mask:
                    raise ValueError(
                        f"{name}.{register_name}.{field_name}: overlapping field"
                    )
                used_mask |= mask
                fields.append(
                    Field(field_name, field_description, offset, width)
                )

            registers.append(
                Register(
                    register_name,
                    register_description,
                    address,
                    access,
                    reset_value,
                    tuple(fields),
                    dim,
                    dim_increment,
                )
            )
        peripherals.append(Peripheral(name, description, base, tuple(registers)))
    if not peripherals:
        raise ValueError("SVD contains no peripherals")
    return tuple(peripherals)


def generate(peripherals: tuple[Peripheral, ...]) -> str:
    access = {
        "read-only": "ReadOnly",
        "write-only": "WriteOnly",
        "read-write": "ReadWrite",
    }
    lines = [
        "//! ESP32-S31 radio clock, power, reset, PBus, PHY-I2C and AGC registers.",
        "//!",
        "//! @generated by `tools/generate-esp32s31-radio-pac.py` from",
        "//! `svd/esp32s31-radio.svd`; edit the SVD, not this file.",
        "",
        "use crate::Register32;",
        "",
    ]
    all_registers: list[str] = []
    for peripheral in peripherals:
        module_name = peripheral.name.lower()
        lines.extend(rust_doc(peripheral.description))
        lines.append(f"pub mod {module_name} {{")
        lines.append("    use crate::{Register32, RegisterAccess};")
        lines.append("")
        lines.extend(rust_doc(f"Peripheral base address. {peripheral.description}", "    "))
        lines.append(f"    pub const BASE: usize = 0x{peripheral.base:08x};")
        lines.append("")
        for register in peripheral.registers:
            rust_name = register.name.replace("%s", "")
            reset = (
                f"Some(0x{register.reset_value:08x})"
                if register.reset_value is not None
                else "None"
            )
            lines.extend(rust_doc(register.description, "    "))
            if register.dim == 1:
                declaration = (
                    f"pub const {rust_name}: Register32 = "
                    f"Register32::described(0x{register.address:08x}, "
                    f"RegisterAccess::{access[register.access]}, {reset});"
                )
                if len(declaration) + 4 <= 100:
                    lines.append(f"    {declaration}")
                else:
                    lines.append(f"    pub const {rust_name}: Register32 =")
                    lines.append(
                        f"        Register32::described(0x{register.address:08x}, "
                        f"RegisterAccess::{access[register.access]}, {reset});"
                    )
                all_registers.append(f"{module_name}::{rust_name}")
            else:
                lines.append(
                    f"    pub const {rust_name}: [Register32; {register.dim}] = ["
                )
                for index in range(register.dim):
                    address_value = (
                        register.address + index * register.dim_increment
                    )
                    lines.append(
                        f"        Register32::described(0x{address_value:08x}, "
                        f"RegisterAccess::{access[register.access]}, {reset}),"
                    )
                    all_registers.append(
                        f"{module_name}::{rust_name}[{index}]"
                    )
                lines.append("    ];")
            if register.fields:
                lines.append("")
                lines.extend(
                    rust_doc(
                        f"Recovered fields of [`{rust_name}`]. "
                        f"{register.description}",
                        "    ",
                    )
                )
                lines.append(f"    pub mod {rust_name.lower()} {{")
                lines.append("        use crate::Field32;")
                lines.append("")
                for field in register.fields:
                    lines.extend(rust_doc(field.description, "        "))
                    lines.append(
                        f"        pub const {field.name}: Field32 = "
                        f"Field32::new({field.offset}, {field.width});"
                    )
                lines.append("    }")
            lines.append("")
        if lines[-1] == "":
            lines.pop()
        lines.append("}")
        lines.append("")
    lines.append("/// Complete generated register allow-list in ascending SVD order.")
    lines.append(f"pub const ALL: [Register32; {len(all_registers)}] = [")
    lines.extend(f"    {name}," for name in all_registers)
    lines.append("];")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if the checked-in Rust output is stale",
    )
    args = parser.parse_args()
    try:
        generated = generate(parse_svd())
    except (ET.ParseError, ValueError) as error:
        print(f"{SVD_PATH}: {error}", file=sys.stderr)
        return 1

    if args.check:
        existing = OUTPUT_PATH.read_text() if OUTPUT_PATH.exists() else ""
        if existing != generated:
            diff = difflib.unified_diff(
                existing.splitlines(),
                generated.splitlines(),
                fromfile=str(OUTPUT_PATH),
                tofile="generated",
                lineterm="",
            )
            print("\n".join(diff), file=sys.stderr)
            return 1
        return 0

    OUTPUT_PATH.write_text(generated)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
