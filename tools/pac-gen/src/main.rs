use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

use roxmltree::{Document, Node};
use svd_parser::svd::{Access, MaybeArray, RegisterCluster, RegisterProperties};
use svd2rust::{
    Target,
    config::{Config, RustEdition},
};

mod radio_svd;

const USAGE: &str = "usage: cargo pac-gen [--check]";
const ALLOWED_CONFIDENCE_VALUES: &[&str] = &[
    "block-exact-register-semantics-opaque",
    "hil-observed",
    "instruction-exact",
    "instruction-exact-hil-qualified",
    "instruction-exact-partial",
    "instruction-exact-semantics-unknown",
];
#[derive(Clone, Debug, Eq, PartialEq)]
struct MmioWindow {
    name: String,
    start: u64,
    end_exclusive: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpandedRegister {
    identity: String,
    peripheral: String,
    scope: Vec<ExpandedScope>,
    name: String,
    rust_name: String,
    array_index: Option<u32>,
    size_bits: u32,
    access: Option<Access>,
    alternate_group: Option<String>,
    alternate_register: Option<String>,
    fields: Vec<ExpandedField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpandedScope {
    identity_name: String,
    rust_name: String,
    array_index: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpandedField {
    name: String,
    rust_name: String,
    array_index: Option<u32>,
    bit_offset: u32,
    bit_width: u32,
    access: Option<Access>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InterruptSnapshotBinding {
    name: String,
    peripheral: String,
    status_register: String,
    clear_register: String,
    clear_field: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FullRegisterWriteBinding {
    name: String,
    peripheral: String,
    register: String,
    field: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FixedRegisterWriteBinding {
    name: String,
    peripheral: String,
    register: String,
    field: String,
    variant: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FixedRegisterImageBinding {
    name: String,
    peripheral: String,
    register: String,
    value: u32,
    register_is_array: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegisterImageWriteBinding {
    name: String,
    peripheral: String,
    register: String,
    register_is_array: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ZeroBasedFieldWriteBinding {
    name: String,
    peripheral: String,
    register: String,
    fields: Vec<ZeroBasedFieldBinding>,
    register_is_array: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ZeroBasedFieldBinding {
    name: String,
    value_type: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ZeroRegisterWriteBinding {
    name: String,
    peripheral: String,
    register: String,
    register_is_array: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MaskedRegisterModifyBinding {
    name: String,
    peripheral: String,
    register: String,
    preserve_mask: u32,
    input_mask: u32,
    set_mask: u32,
    register_is_array: bool,
}

const fn inherited_properties(
    parent: RegisterProperties,
    child: RegisterProperties,
) -> RegisterProperties {
    let mut properties = parent;
    if child.size.is_some() {
        properties.size = child.size;
    }
    if child.access.is_some() {
        properties.access = child.access;
    }
    if child.protection.is_some() {
        properties.protection = child.protection;
    }
    if child.reset_value.is_some() {
        properties.reset_value = child.reset_value;
    }
    if child.reset_mask.is_some() {
        properties.reset_mask = child.reset_mask;
    }
    properties
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("pac-gen must remain under tools/pac-gen")
        .to_owned()
}

fn format_generated(source: &str) -> Result<String, Box<dyn Error>> {
    let source = format!("#![allow(clippy::empty_docs)]\n{source}");
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--style-edition", "2024"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("piped rustfmt stdin must exist")
        .write_all(source.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(format!("rustfmt exited with {}", output.status).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn mmio_window(windows: &[MmioWindow], start: u64, end_exclusive: u64) -> Option<&str> {
    windows
        .iter()
        .find(|window| start >= window.start && end_exclusive <= window.end_exclusive)
        .map(|window| window.name.as_str())
}

fn register_size_bytes(
    properties: RegisterProperties,
    inherited_size_bits: Option<u32>,
) -> Result<u64, Box<dyn Error>> {
    let size_bits = properties
        .size
        .or(inherited_size_bits)
        .ok_or("SVD register has no size and no inherited size")?;
    if size_bits == 0 {
        return Err("SVD register size must not be zero".into());
    }
    Ok(u64::from(size_bits).div_ceil(8))
}

fn validate_register(
    windows: &[MmioWindow],
    peripheral_name: &str,
    register_name: &str,
    peripheral_base: u64,
    parent_offset: u64,
    register_offset: u32,
    properties: RegisterProperties,
    inherited_size_bits: Option<u32>,
) -> Result<(), Box<dyn Error>> {
    let start = peripheral_base
        .checked_add(parent_offset)
        .and_then(|address| address.checked_add(u64::from(register_offset)))
        .ok_or("SVD register address overflow")?;
    let end_exclusive = start
        .checked_add(register_size_bytes(properties, inherited_size_bits)?)
        .ok_or("SVD register end address overflow")?;
    if mmio_window(windows, start, end_exclusive).is_none() {
        return Err(format!(
            "SVD register {peripheral_name}.{register_name} spans \
             0x{start:08x}..0x{end_exclusive:08x}, outside the evidenced \
             ESP32-S31 MMIO windows"
        )
        .into());
    }
    Ok(())
}

fn validate_children(
    windows: &[MmioWindow],
    peripheral_name: &str,
    peripheral_base: u64,
    parent_offset: u64,
    children: &[RegisterCluster],
    inherited_size_bits: Option<u32>,
) -> Result<(), Box<dyn Error>> {
    for child in children {
        match child {
            RegisterCluster::Register(register) => match register {
                MaybeArray::Single(info) => validate_register(
                    windows,
                    peripheral_name,
                    &info.name,
                    peripheral_base,
                    parent_offset,
                    info.address_offset,
                    info.properties,
                    inherited_size_bits,
                )?,
                MaybeArray::Array(info, dim) => {
                    for index in 0..dim.dim {
                        let offset = info
                            .address_offset
                            .checked_add(index.saturating_mul(dim.dim_increment))
                            .ok_or("SVD register-array offset overflow")?;
                        validate_register(
                            windows,
                            peripheral_name,
                            &info.name,
                            peripheral_base,
                            parent_offset,
                            offset,
                            info.properties,
                            inherited_size_bits,
                        )?;
                    }
                }
            },
            RegisterCluster::Cluster(cluster) => {
                let cluster_size = cluster
                    .default_register_properties
                    .size
                    .or(inherited_size_bits);
                match cluster {
                    MaybeArray::Single(info) => validate_children(
                        windows,
                        peripheral_name,
                        peripheral_base,
                        parent_offset + u64::from(info.address_offset),
                        &info.children,
                        cluster_size,
                    )?,
                    MaybeArray::Array(info, dim) => {
                        for index in 0..dim.dim {
                            let offset = info
                                .address_offset
                                .checked_add(index.saturating_mul(dim.dim_increment))
                                .ok_or("SVD cluster-array offset overflow")?;
                            validate_children(
                                windows,
                                peripheral_name,
                                peripheral_base,
                                parent_offset + u64::from(offset),
                                &info.children,
                                cluster_size,
                            )?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_mmio_windows(input: &str, windows: &[MmioWindow]) -> Result<(), Box<dyn Error>> {
    let device = svd_parser::parse(input)?;
    for peripheral in &device.peripherals {
        let peripheral_size = peripheral
            .default_register_properties
            .size
            .or(device.default_register_properties.size);
        let validate_instance = |base_address: u64| -> Result<(), Box<dyn Error>> {
            if mmio_window(windows, base_address, base_address + 1).is_none() {
                return Err(format!(
                    "SVD peripheral {} starts at 0x{base_address:08x}, outside \
                     the evidenced ESP32-S31 MMIO windows",
                    peripheral.name
                )
                .into());
            }
            if let Some(registers) = &peripheral.registers {
                validate_children(
                    windows,
                    &peripheral.name,
                    base_address,
                    0,
                    registers,
                    peripheral_size,
                )?;
            }
            Ok(())
        };

        match peripheral {
            MaybeArray::Single(info) => validate_instance(info.base_address)?,
            MaybeArray::Array(info, dim) => {
                for index in 0..dim.dim {
                    validate_instance(
                        info.base_address + u64::from(index.saturating_mul(dim.dim_increment)),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn parse_u64(value: &str, what: &str) -> Result<u64, Box<dyn Error>> {
    let value = value.trim();
    let parsed = if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16)
    } else if let Some(binary) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
        .or_else(|| value.strip_prefix('#'))
    {
        u64::from_str_radix(binary, 2)
    } else {
        value.parse()
    };
    parsed.map_err(|error| format!("invalid {what} `{value}`: {error}").into())
}

fn child_text<'a>(node: Node<'a, 'a>, name: &str) -> Result<&'a str, Box<dyn Error>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
        .and_then(|child| child.text())
        .ok_or_else(|| format!("{} has no {name}", node.tag_name().name()).into())
}

fn child_u64(node: Node<'_, '_>, name: &str) -> Result<u64, Box<dyn Error>> {
    parse_u64(child_text(node, name)?, name)
}

fn optional_child_text<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
        .and_then(|child| child.text())
}

fn inherited_child_text<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.ancestors()
        .find_map(|ancestor| optional_child_text(ancestor, name))
}

fn parse_mmio_windows(document: &Document<'_>) -> Result<Vec<MmioWindow>, Box<dyn Error>> {
    let container = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioAddressWindows"))
        .ok_or("SVD has no openEspRadioAddressWindows vendor extension")?;
    let mut windows = Vec::new();
    for node in container
        .children()
        .filter(|node| node.has_tag_name("window"))
    {
        let name = node.attribute("name").ok_or("MMIO window has no name")?;
        let start = parse_u64(
            node.attribute("start").ok_or("MMIO window has no start")?,
            "MMIO window start",
        )?;
        let end_exclusive = parse_u64(
            node.attribute("endExclusive")
                .ok_or("MMIO window has no endExclusive")?,
            "MMIO window endExclusive",
        )?;
        if start >= end_exclusive {
            return Err(format!(
                "MMIO window {name} has invalid range 0x{start:08x}..0x{end_exclusive:08x}"
            )
            .into());
        }
        windows.push(MmioWindow {
            name: name.to_owned(),
            start,
            end_exclusive,
        });
    }
    if windows.is_empty() {
        return Err("openEspRadioAddressWindows contains no windows".into());
    }
    windows.sort_by_key(|window| window.start);
    for pair in windows.windows(2) {
        if pair[0].end_exclusive > pair[1].start {
            return Err(
                format!("MMIO windows {} and {} overlap", pair[0].name, pair[1].name).into(),
            );
        }
    }
    Ok(windows)
}

fn validate_evidence_ranges(
    document: &Document<'_>,
    windows: &[MmioWindow],
) -> Result<(), Box<dyn Error>> {
    let Some(container) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioEvidenceRanges"))
    else {
        return Ok(());
    };
    let mut names = BTreeSet::new();
    let mut ranges = Vec::new();
    for node in container
        .children()
        .filter(|node| node.has_tag_name("range"))
    {
        let name = node.attribute("name").ok_or("evidence range has no name")?;
        if !names.insert(name) {
            return Err(format!("duplicate evidence range name {name}").into());
        }
        let start = parse_u64(
            node.attribute("start")
                .ok_or("evidence range has no start")?,
            "evidence range start",
        )?;
        let end_exclusive = parse_u64(
            node.attribute("endExclusive")
                .ok_or("evidence range has no endExclusive")?,
            "evidence range endExclusive",
        )?;
        if start >= end_exclusive {
            return Err(format!(
                "evidence range {name} has invalid half-open bounds \
                 0x{start:08x}..0x{end_exclusive:08x}"
            )
            .into());
        }
        if start % 4 != 0 || end_exclusive % 4 != 0 {
            return Err(format!("evidence range {name} is not word-aligned").into());
        }
        if mmio_window(windows, start, end_exclusive).is_none() {
            return Err(
                format!("evidence range {name} lies outside the evidenced MMIO windows").into(),
            );
        }
        ranges.push((start, end_exclusive, name));
    }
    ranges.sort_by_key(|range| range.0);
    for pair in ranges.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(format!("evidence ranges {} and {} overlap", pair[0].2, pair[1].2).into());
        }
    }
    Ok(())
}

fn validate_dimension_order(document: &Document<'_>) -> Result<(), Box<dyn Error>> {
    for node in document.descendants().filter(|node| {
        node.is_element()
            && matches!(
                node.tag_name().name(),
                "peripheral" | "cluster" | "register" | "field"
            )
    }) {
        let element_names = node
            .children()
            .filter(|child| child.is_element())
            .map(|child| child.tag_name().name())
            .collect::<Vec<_>>();
        let Some(dim_position) = element_names.iter().position(|name| *name == "dim") else {
            continue;
        };
        let name_position = element_names
            .iter()
            .position(|name| *name == "name")
            .ok_or("dimensioned SVD element has no name")?;
        if dim_position > name_position {
            return Err(format!(
                "{} {} places dim after name; CMSIS-SVD requires the dim group first",
                node.tag_name().name(),
                child_text(node, "name")?
            )
            .into());
        }
        let mut previous_schema_position = None;
        let invalid_dimension_child = element_names[..name_position].iter().any(|name| {
            let schema_position = match *name {
                "dim" => 0,
                "dimIncrement" => 1,
                "dimIndex" => 2,
                "dimName" => 3,
                "dimArrayIndex" => 4,
                _ => usize::MAX,
            };
            let invalid = schema_position == usize::MAX
                || previous_schema_position.is_some_and(|previous| previous >= schema_position);
            previous_schema_position = Some(schema_position);
            invalid
        });
        if dim_position != 0
            || element_names.get(1) != Some(&"dimIncrement")
            || invalid_dimension_child
        {
            return Err(format!(
                "{} {} has a non-canonical CMSIS-SVD dimension group",
                node.tag_name().name(),
                child_text(node, "name")?
            )
            .into());
        }
    }
    Ok(())
}

fn validate_register_layout(document: &Document<'_>) -> Result<(), Box<dyn Error>> {
    for register in document
        .descendants()
        .filter(|node| node.has_tag_name("register"))
    {
        let register_name = child_text(register, "name")?;
        let size_bits = parse_u64(
            inherited_child_text(register, "size")
                .ok_or_else(|| format!("register {register_name} has no inherited size"))?,
            "register size",
        )?;
        if size_bits == 0 || size_bits > 128 {
            return Err(
                format!("register {register_name} has unsupported size {size_bits}").into(),
            );
        }
        let size_bytes = size_bits.div_ceil(8);
        let address_offset = parse_u64(
            child_text(register, "addressOffset")?,
            "register addressOffset",
        )?;
        if address_offset % size_bytes != 0 {
            return Err(format!(
                "register {register_name} offset 0x{address_offset:x} is not aligned to {size_bytes} bytes"
            )
            .into());
        }
        if let Some(increment) = optional_child_text(register, "dimIncrement") {
            let increment = parse_u64(increment, "register dimIncrement")?;
            if increment < size_bytes {
                return Err(format!(
                    "register array {register_name} has dimIncrement {increment}, smaller than its {size_bytes}-byte element"
                )
                .into());
            }
            if increment % size_bytes != 0 {
                return Err(format!(
                    "register array {register_name} has unaligned dimIncrement {increment} for its {size_bytes}-byte element"
                )
                .into());
            }
        }

        let Some(fields) = register.children().find(|node| node.has_tag_name("fields")) else {
            continue;
        };
        let mut occupied = 0_u128;
        let mut field_names = BTreeSet::new();
        for field in fields.children().filter(|node| node.has_tag_name("field")) {
            let field_name = child_text(field, "name")?;
            if field_name.contains("PRESERVED") {
                return Err(format!(
                    "register {register_name} exposes filler field {field_name}; omit preserved/reserved bits instead"
                )
                .into());
            }
            if !field_names.insert(field_name) {
                return Err(format!(
                    "register {register_name} contains duplicate field name {field_name}"
                )
                .into());
            }
            let offset = parse_u64(child_text(field, "bitOffset")?, "field bitOffset")?;
            let width = parse_u64(child_text(field, "bitWidth")?, "field bitWidth")?;
            if width == 0 || offset.checked_add(width).is_none_or(|end| end > size_bits) {
                return Err(format!(
                    "register {register_name} field {field_name} has invalid bit range {offset}+{width} for a {size_bits}-bit register"
                )
                .into());
            }
            let mask = if width == 128 {
                u128::MAX
            } else {
                ((1_u128 << width) - 1) << offset
            };
            if occupied & mask != 0 {
                return Err(format!(
                    "register {register_name} field {field_name} overlaps another field"
                )
                .into());
            }
            occupied |= mask;
        }
    }
    Ok(())
}

fn maximum_unsigned_value(width: u64) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1_u64 << width) - 1
    }
}

fn exact_enumerated_value(
    value: &str,
    width: u64,
    identity: &str,
) -> Result<Option<u64>, Box<dyn Error>> {
    let value = value.trim();
    let binary = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
        .or_else(|| value.strip_prefix('#'));
    if let Some(binary) = binary
        && binary.bytes().any(|byte| matches!(byte, b'x' | b'X'))
    {
        if binary.len() as u64 > width
            || binary
                .bytes()
                .any(|byte| !matches!(byte, b'0' | b'1' | b'x' | b'X'))
        {
            return Err(format!(
                "field {identity} has invalid {width}-bit enumerated pattern {value}"
            )
            .into());
        }
        return Ok(None);
    }
    Ok(Some(parse_u64(value, "enumerated value")?))
}

fn validate_write_semantics(document: &Document<'_>) -> Result<(), Box<dyn Error>> {
    for field in document
        .descendants()
        .filter(|node| node.has_tag_name("field"))
    {
        let field_name = child_text(field, "name")?;
        let register_name = field
            .ancestors()
            .find(|node| node.has_tag_name("register"))
            .map(|register| child_text(register, "name"))
            .transpose()?
            .unwrap_or("<unknown-register>");
        let identity = format!("{register_name}.{field_name}");
        let width = parse_u64(child_text(field, "bitWidth")?, "field bitWidth")?;
        let maximum = maximum_unsigned_value(width);

        let enumerations = field
            .children()
            .filter(|node| node.has_tag_name("enumeratedValues"))
            .collect::<Vec<_>>();
        for enumeration in &enumerations {
            let mut names = BTreeSet::new();
            let mut values = BTreeSet::new();
            let mut default_count = 0;
            for variant in enumeration
                .children()
                .filter(|node| node.has_tag_name("enumeratedValue"))
            {
                let name = child_text(variant, "name")?;
                if !names.insert(name) {
                    return Err(format!(
                        "field {identity} contains duplicate enumerated name {name}"
                    )
                    .into());
                }
                if optional_child_text(variant, "isDefault") == Some("true") {
                    default_count += 1;
                }
                let Some(value) = optional_child_text(variant, "value") else {
                    continue;
                };
                let Some(value) = exact_enumerated_value(value, width, &identity)? else {
                    continue;
                };
                if value > maximum {
                    return Err(format!(
                        "field {identity} enumerated value {value} exceeds its {width}-bit width"
                    )
                    .into());
                }
                if !values.insert(value) {
                    return Err(format!(
                        "field {identity} contains duplicate enumerated value {value}"
                    )
                    .into());
                }
            }
            if default_count > 1 {
                return Err(format!(
                    "field {identity} contains more than one default enumerated value"
                )
                .into());
            }
        }

        let Some(constraint) = field
            .children()
            .find(|node| node.has_tag_name("writeConstraint"))
        else {
            continue;
        };
        if let Some(use_enumerated) = optional_child_text(constraint, "useEnumeratedValues") {
            if use_enumerated != "true" {
                return Err(format!(
                    "field {identity} has non-operative useEnumeratedValues={use_enumerated}"
                )
                .into());
            }
            let has_write_enumeration = enumerations.iter().any(|enumeration| {
                matches!(
                    optional_child_text(*enumeration, "usage"),
                    None | Some("write" | "read-write")
                )
            });
            if !has_write_enumeration {
                return Err(format!(
                    "field {identity} requires enumerated writes but defines no write enumeration"
                )
                .into());
            }
        }
        if let Some(write_as_read) = optional_child_text(constraint, "writeAsRead")
            && write_as_read != "true"
        {
            return Err(
                format!("field {identity} has non-operative writeAsRead={write_as_read}").into(),
            );
        }
        if let Some(range) = constraint
            .children()
            .find(|node| node.has_tag_name("range"))
        {
            let minimum = parse_u64(child_text(range, "minimum")?, "write minimum")?;
            let maximum_constraint = parse_u64(child_text(range, "maximum")?, "write maximum")?;
            if minimum > maximum_constraint || maximum_constraint > maximum {
                return Err(format!(
                    "field {identity} has invalid write range {minimum}..={maximum_constraint} for its {width}-bit width"
                )
                .into());
            }
        }
    }
    Ok(())
}

fn validate_names(document: &Document<'_>) -> Result<(), Box<dyn Error>> {
    let peripherals = document
        .descendants()
        .find(|node| node.has_tag_name("peripherals"))
        .ok_or("SVD has no peripherals element")?;
    let mut peripheral_names = BTreeSet::new();
    for peripheral in peripherals
        .children()
        .filter(|node| node.has_tag_name("peripheral"))
    {
        let name = child_text(peripheral, "name")?;
        if !peripheral_names.insert(name) {
            return Err(format!("duplicate peripheral name {name}").into());
        }
    }

    for scope in document
        .descendants()
        .filter(|node| node.has_tag_name("registers") || node.has_tag_name("cluster"))
    {
        let mut names = BTreeSet::new();
        for child in scope
            .children()
            .filter(|node| node.has_tag_name("register") || node.has_tag_name("cluster"))
        {
            let name = child_text(child, "name")?;
            if !names.insert(name) {
                return Err(
                    format!("duplicate register/cluster name {name} in one SVD scope").into(),
                );
            }
        }
    }
    Ok(())
}

fn annotation_references<'a>(
    text: &'a str,
    annotation: &str,
) -> Result<Vec<&'a str>, Box<dyn Error>> {
    let prefix = format!("{annotation}[");
    let mut references = BTreeSet::new();
    let mut remaining = text;
    while let Some(start) = remaining.find(&prefix) {
        remaining = &remaining[start + prefix.len()..];
        let end = remaining
            .find(']')
            .ok_or_else(|| format!("unterminated {annotation} annotation"))?;
        for source in remaining[..end].split(',').map(str::trim) {
            if !source.is_empty() {
                references.insert(source);
            }
        }
        remaining = &remaining[end + 1..];
    }
    Ok(references.into_iter().collect())
}

fn validate_provenance(document: &Document<'_>, input: &str) -> Result<(), Box<dyn Error>> {
    let mut definitions = BTreeSet::new();
    for source in document
        .descendants()
        .filter(|node| node.has_tag_name("source"))
    {
        let id = source
            .attribute("id")
            .ok_or("provenance source has no id")?;
        if !definitions.insert(id) {
            return Err(format!("duplicate provenance source id {id}").into());
        }
        if source.text().is_none_or(|text| text.trim().is_empty()) {
            return Err(format!("provenance source {id} has no description").into());
        }
    }
    for reference in annotation_references(input, "SOURCE")? {
        if !definitions.contains(reference) {
            return Err(format!("SOURCE references undefined provenance id {reference}").into());
        }
    }
    for extension in ["openEspRadioAddressWindows", "openEspRadioEvidenceRanges"] {
        let Some(node) = document
            .descendants()
            .find(|node| node.has_tag_name(extension))
        else {
            continue;
        };
        let source = node
            .attribute("source")
            .ok_or_else(|| format!("{extension} has no source"))?;
        if !definitions.contains(source) {
            return Err(format!("{extension} references undefined provenance id {source}").into());
        }
    }
    if let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioInterruptSnapshots"))
    {
        for snapshot in extension
            .children()
            .filter(|node| node.has_tag_name("snapshot"))
        {
            let sources = required_attribute(snapshot, "source")?;
            for source in sources.split(',').map(str::trim) {
                if source.is_empty() || !definitions.contains(source) {
                    return Err(format!(
                        "interrupt snapshot {} references undefined provenance source {source:?}",
                        required_attribute(snapshot, "name")?
                    )
                    .into());
                }
            }
        }
    }
    if let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioFullRegisterWrites"))
    {
        for write in extension
            .children()
            .filter(|node| node.has_tag_name("write"))
        {
            let sources = required_attribute(write, "source")?;
            for source in sources.split(',').map(str::trim) {
                if source.is_empty() || !definitions.contains(source) {
                    return Err(format!(
                        "full-register write {} references undefined provenance source {source:?}",
                        required_attribute(write, "name")?
                    )
                    .into());
                }
            }
        }
    }
    if let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioFixedRegisterWrites"))
    {
        for write in extension
            .children()
            .filter(|node| node.has_tag_name("write"))
        {
            let sources = required_attribute(write, "source")?;
            for source in sources.split(',').map(str::trim) {
                if source.is_empty() || !definitions.contains(source) {
                    return Err(format!(
                        "fixed-register write {} references undefined provenance source {source:?}",
                        required_attribute(write, "name")?
                    )
                    .into());
                }
            }
        }
    }
    for (extension_name, operation_name) in [
        ("openEspRadioFixedRegisterImages", "fixed-register image"),
        ("openEspRadioRegisterImageWrites", "register-image write"),
    ] {
        if let Some(extension) = document
            .descendants()
            .find(|node| node.has_tag_name(extension_name))
        {
            for write in extension
                .children()
                .filter(|node| node.has_tag_name("write"))
            {
                let sources = required_attribute(write, "source")?;
                for source in sources.split(',').map(str::trim) {
                    if source.is_empty() || !definitions.contains(source) {
                        return Err(format!(
                            "{operation_name} {} references undefined provenance source {source:?}",
                            required_attribute(write, "name")?
                        )
                        .into());
                    }
                }
            }
        }
    }
    if let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioZeroBasedFieldWrites"))
    {
        for write in extension
            .children()
            .filter(|node| node.has_tag_name("write"))
        {
            let sources = required_attribute(write, "source")?;
            for source in sources.split(',').map(str::trim) {
                if source.is_empty() || !definitions.contains(source) {
                    return Err(format!(
                        "zero-based field write {} references undefined provenance source {source:?}",
                        required_attribute(write, "name")?
                    )
                    .into());
                }
            }
        }
    }
    if let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioZeroRegisterWrites"))
    {
        for write in extension
            .children()
            .filter(|node| node.has_tag_name("write"))
        {
            let sources = required_attribute(write, "source")?;
            for source in sources.split(',').map(str::trim) {
                if source.is_empty() || !definitions.contains(source) {
                    return Err(format!(
                        "zero-register write {} references undefined provenance source {source:?}",
                        required_attribute(write, "name")?
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}

fn validate_model_review_sources(
    document: &Document<'_>,
    review_sources: &BTreeSet<String>,
) -> Result<(), Box<dyn Error>> {
    let definitions = document
        .descendants()
        .filter(|node| node.has_tag_name("source"))
        .filter_map(|node| node.attribute("id"))
        .collect::<BTreeSet<_>>();
    if let Some(source) = review_sources
        .iter()
        .find(|source| !definitions.contains(source.as_str()))
    {
        return Err(format!(
            "register model review references undefined PAC add-on provenance source {source}"
        )
        .into());
    }
    Ok(())
}

fn validate_confidence(input: &str) -> Result<(), Box<dyn Error>> {
    let allowed = ALLOWED_CONFIDENCE_VALUES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    for value in annotation_references(input, "CONFIDENCE")? {
        if !allowed.contains(value) {
            return Err(format!(
                "CONFIDENCE references unsupported value {value}; allowed values are {allowed:?}"
            )
            .into());
        }
    }
    Ok(())
}

fn remove_dimension_placeholder(value: &str) -> String {
    value.replace("[%s]", "").replace("%s", "")
}

fn member_binding_name(value: &str) -> String {
    remove_dimension_placeholder(value).to_ascii_lowercase()
}

fn array_binding_name(value: &str, dim_name: Option<&str>) -> String {
    member_binding_name(dim_name.unwrap_or(value))
}

fn type_binding_name(value: &str) -> String {
    let value = remove_dimension_placeholder(value);
    let mut output = String::new();
    let mut capitalize = true;
    for character in value.chars() {
        if character == '_' || character == '-' {
            capitalize = true;
        } else if capitalize {
            output.push(character.to_ascii_uppercase());
            capitalize = false;
        } else {
            output.push(character.to_ascii_lowercase());
        }
    }
    output
}

fn required_attribute<'a>(node: Node<'a, 'a>, name: &str) -> Result<&'a str, Box<dyn Error>> {
    node.attribute(name)
        .ok_or_else(|| format!("{} has no {name} attribute", node.tag_name().name()).into())
}

fn direct_named_child<'a>(parent: Node<'a, 'a>, tag: &str, name: &str) -> Option<Node<'a, 'a>> {
    parent.children().find(|node| {
        node.has_tag_name(tag)
            && optional_child_text(*node, "name").is_some_and(|value| value == name)
    })
}

fn parse_interrupt_snapshots(
    document: &Document<'_>,
) -> Result<Vec<InterruptSnapshotBinding>, Box<dyn Error>> {
    let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioInterruptSnapshots"))
    else {
        return Ok(Vec::new());
    };
    if document
        .descendants()
        .filter(|node| node.has_tag_name("openEspRadioInterruptSnapshots"))
        .count()
        != 1
    {
        return Err("SVD has duplicate openEspRadioInterruptSnapshots extensions".into());
    }

    let peripherals = document
        .descendants()
        .find(|node| node.has_tag_name("peripherals"))
        .ok_or("SVD has no peripherals element")?;
    let mut names = BTreeSet::new();
    let mut bindings = Vec::new();
    for snapshot in extension
        .children()
        .filter(|node| node.has_tag_name("snapshot"))
    {
        let name = required_attribute(snapshot, "name")?;
        if name.is_empty()
            || member_binding_name(name) != name
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
            || !name.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(format!("interrupt snapshot name {name:?} is not lower snake case").into());
        }
        if !names.insert(name) {
            return Err(format!("duplicate interrupt snapshot name {name}").into());
        }

        let peripheral_name = required_attribute(snapshot, "peripheral")?;
        let status_name = required_attribute(snapshot, "statusRegister")?;
        let clear_name = required_attribute(snapshot, "clearRegister")?;
        let clear_field_name = required_attribute(snapshot, "clearField")?;
        required_attribute(snapshot, "source")?;

        let peripheral = direct_named_child(peripherals, "peripheral", peripheral_name)
            .ok_or_else(|| {
                format!("interrupt snapshot {name} references unknown peripheral {peripheral_name}")
            })?;
        let registers = peripheral
            .children()
            .find(|node| node.has_tag_name("registers"))
            .ok_or_else(|| format!("peripheral {peripheral_name} has no registers"))?;
        let status = direct_named_child(registers, "register", status_name).ok_or_else(|| {
            format!("interrupt snapshot {name} references unknown status register {status_name}")
        })?;
        let clear = direct_named_child(registers, "register", clear_name).ok_or_else(|| {
            format!("interrupt snapshot {name} references unknown clear register {clear_name}")
        })?;
        if child_u64(status, "size")? != 32 || child_text(status, "access")? != "read-only" {
            return Err(format!(
                "interrupt snapshot {name} status must be a 32-bit read-only register"
            )
            .into());
        }
        if child_u64(clear, "size")? != 32
            || child_text(clear, "modifiedWriteValues")? != "oneToClear"
        {
            return Err(format!(
                "interrupt snapshot {name} clear must be a 32-bit one-to-clear register"
            )
            .into());
        }
        let fields = clear
            .children()
            .find(|node| node.has_tag_name("fields"))
            .ok_or_else(|| format!("interrupt snapshot {name} clear has no fields"))?;
        let clear_field =
            direct_named_child(fields, "field", clear_field_name).ok_or_else(|| {
                format!(
                    "interrupt snapshot {name} references unknown clear field {clear_field_name}"
                )
            })?;
        if child_u64(clear_field, "bitOffset")? != 0 || child_u64(clear_field, "bitWidth")? != 32 {
            return Err(format!(
                "interrupt snapshot {name} clear field must cover the complete 32-bit register"
            )
            .into());
        }

        bindings.push(InterruptSnapshotBinding {
            name: name.to_owned(),
            peripheral: peripheral_name.to_owned(),
            status_register: status_name.to_owned(),
            clear_register: clear_name.to_owned(),
            clear_field: clear_field_name.to_owned(),
        });
    }
    Ok(bindings)
}

fn generate_interrupt_snapshot_api(document: &Document<'_>) -> Result<String, Box<dyn Error>> {
    let bindings = parse_interrupt_snapshots(document)?;
    if bindings.is_empty() {
        return Ok(String::new());
    }

    let mut output = String::from(
        "\n/// Safe, SVD-declared read-and-acknowledge interrupt transactions.\n\
         pub mod interrupt_snapshot {\n",
    );
    for binding in bindings {
        let snapshot_type = format!("{}Snapshot", type_binding_name(&binding.name));
        let peripheral_type = type_binding_name(&binding.peripheral);
        let status = member_binding_name(&binding.status_register);
        let clear = member_binding_name(&binding.clear_register);
        let clear_field = member_binding_name(&binding.clear_field);
        output.push_str(&format!(
            "\n    /// Opaque event image sampled from `{}`.`{}`.\n\
             #[must_use = \"an interrupt snapshot must be inspected and acknowledged\"]\n\
             #[derive(Debug)]\n\
             pub struct {snapshot_type}(u32);\n\
             impl {snapshot_type} {{\n\
                 /// Complete masked event image observed by the status read.\n\
                 #[inline]\n\
                 pub const fn bits(&self) -> u32 {{ self.0 }}\n\
             }}\n\
             /// Sample the complete masked event image.\n\
             #[inline]\n\
             pub fn sample_{}(registers: &crate::{peripheral_type}) -> {snapshot_type} {{\n\
                 {snapshot_type}(registers.{status}().read().bits())\n\
             }}\n\
             /// Acknowledge exactly the event image returned by the paired sample.\n\
             #[inline]\n\
             pub fn acknowledge_{}(\n\
                 registers: &crate::{peripheral_type},\n\
                 snapshot: {snapshot_type},\n\
             ) {{\n\
                 // SAFETY: the opaque value can only be constructed by the paired\n\
                 // STATUS read (or in a validation-only build) and CLEAR is an\n\
                 // SVD-validated full-width write-one-to-clear register.\n\
                 unsafe {{\n\
                     registers.{clear}().write_with_zero(|writer|\n\
                         writer.{clear_field}().bits(snapshot.0)\n\
                     );\n\
                 }}\n\
             }}\n\
             #[cfg(feature = \"validation-probes\")]\n\
             #[doc(hidden)]\n\
             pub const fn {}_for_validation(bits: u32) -> {snapshot_type} {{\n\
                 {snapshot_type}(bits)\n\
             }}\n",
            binding.peripheral, binding.status_register, binding.name, binding.name, binding.name,
        ));
    }
    output.push_str("}\n");
    Ok(output)
}

fn generate_peripheral_ownership_api(document: &Document<'_>) -> Result<String, Box<dyn Error>> {
    let interrupt_peripherals = parse_interrupt_snapshots(document)?
        .into_iter()
        .map(|binding| binding.peripheral)
        .collect::<BTreeSet<_>>();
    if interrupt_peripherals.is_empty() {
        return Ok(String::new());
    }

    let peripherals = document
        .descendants()
        .find(|node| node.has_tag_name("peripherals"))
        .ok_or("SVD has no peripherals element")?;
    let peripheral_names = peripherals
        .children()
        .filter(|node| node.has_tag_name("peripheral"))
        .map(|node| child_text(node, "name").map(str::to_owned))
        .collect::<Result<Vec<_>, _>>()?;
    let ordinary_peripherals = peripheral_names
        .iter()
        .filter(|name| !interrupt_peripherals.contains(name.as_str()))
        .collect::<Vec<_>>();
    let interrupt_peripherals = peripheral_names
        .iter()
        .filter(|name| interrupt_peripherals.contains(name.as_str()))
        .collect::<Vec<_>>();

    let fields = |names: &[&String]| {
        names
            .iter()
            .map(|name| {
                format!(
                    "    pub {}: crate::{},\n",
                    member_binding_name(name),
                    type_binding_name(name),
                )
            })
            .collect::<String>()
    };
    let members = |names: &[&String]| {
        names
            .iter()
            .map(|name| format!("        {},\n", member_binding_name(name)))
            .collect::<String>()
    };

    let all_members = members(&peripheral_names.iter().collect::<Vec<_>>());
    let ordinary_members = members(&ordinary_peripherals);
    let interrupt_members = members(&interrupt_peripherals);
    Ok(format!(
        "\n/// Safe ownership partitions derived from the SVD interrupt banks.\n\
         pub mod peripheral_ownership {{\n\
         /// Radio peripherals which remain available to ordinary task code.\n\
         #[allow(non_snake_case)]\n\
         pub struct RadioPeripherals {{\n{}         }}\n\
         /// Interrupt banks transferred from cold setup to the hard handlers.\n\
         #[allow(non_snake_case)]\n\
         pub struct InterruptPeripherals {{\n{}         }}\n\
         /// Consume the singleton and separate task-owned registers from interrupt banks.\n\
         #[inline]\n\
         pub fn split(\n\
             peripherals: crate::Peripherals,\n\
         ) -> (RadioPeripherals, InterruptPeripherals) {{\n\
             let crate::Peripherals {{\n{}             }} = peripherals;\n\
             (\n\
                 RadioPeripherals {{\n{}                 }},\n\
                 InterruptPeripherals {{\n{}                 }},\n\
             )\n\
         }}\n\
         /// Acquire a fresh singleton in an isolated compiled-validation image.\n\
         #[cfg(feature = \"validation-probes\")]\n\
         #[doc(hidden)]\n\
         #[inline]\n\
         pub fn peripherals_for_validation() -> crate::Peripherals {{\n\
             // SAFETY: validation images contain one closed probe and no runtime driver.\n\
             unsafe {{ crate::Peripherals::steal() }}\n\
         }}\n\
         }}\n",
        fields(&ordinary_peripherals),
        fields(&interrupt_peripherals),
        all_members,
        ordinary_members,
        interrupt_members,
    ))
}

fn generate_device_access_api() -> &'static str {
    "\n/// Architecture-specific device-memory ordering primitives.\n\
     pub mod device_access {\n\
         /// Order all preceding and following device-memory accesses.\n\
         #[inline]\n\
         pub fn fence() {\n\
             #[cfg(target_arch = \"riscv32\")]\n\
             // SAFETY: this instruction only orders memory and device accesses.\n\
             unsafe { core::arch::asm!(\"fence iorw, iorw\") }\n\
             #[cfg(target_arch = \"arm\")]\n\
             // SAFETY: this instruction only orders memory and device accesses.\n\
             unsafe { core::arch::asm!(\"dmb sy\") }\n\
             #[cfg(target_arch = \"xtensa\")]\n\
             // SAFETY: this instruction only orders memory and device accesses.\n\
             unsafe { core::arch::asm!(\"memw\") }\n\
             #[cfg(not(any(\n\
                 target_arch = \"riscv32\",\n\
                 target_arch = \"arm\",\n\
                 target_arch = \"xtensa\",\n\
             )))]\n\
             core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);\n\
         }\n\
     }\n"
}

fn parse_full_register_writes(
    document: &Document<'_>,
) -> Result<Vec<FullRegisterWriteBinding>, Box<dyn Error>> {
    let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioFullRegisterWrites"))
    else {
        return Ok(Vec::new());
    };
    if document
        .descendants()
        .filter(|node| node.has_tag_name("openEspRadioFullRegisterWrites"))
        .count()
        != 1
    {
        return Err("SVD has duplicate openEspRadioFullRegisterWrites extensions".into());
    }

    let peripherals = document
        .descendants()
        .find(|node| node.has_tag_name("peripherals"))
        .ok_or("SVD has no peripherals element")?;
    let mut names = BTreeSet::new();
    let mut bindings = Vec::new();
    for write in extension
        .children()
        .filter(|node| node.has_tag_name("write"))
    {
        let name = required_attribute(write, "name")?;
        if name.is_empty()
            || member_binding_name(name) != name
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
            || !name.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(
                format!("full-register write name {name:?} is not lower snake case").into(),
            );
        }
        if !names.insert(name) {
            return Err(format!("duplicate full-register write name {name}").into());
        }

        let peripheral_name = required_attribute(write, "peripheral")?;
        let register_name = required_attribute(write, "register")?;
        let field_name = required_attribute(write, "field")?;
        required_attribute(write, "source")?;
        let peripheral = direct_named_child(peripherals, "peripheral", peripheral_name)
            .ok_or_else(|| {
                format!(
                    "full-register write {name} references unknown peripheral {peripheral_name}"
                )
            })?;
        let registers = peripheral
            .children()
            .find(|node| node.has_tag_name("registers"))
            .ok_or_else(|| format!("peripheral {peripheral_name} has no registers"))?;
        let register =
            direct_named_child(registers, "register", register_name).ok_or_else(|| {
                format!("full-register write {name} references unknown register {register_name}")
            })?;
        let access = child_text(register, "access")?;
        if child_u64(register, "size")? != 32 || !matches!(access, "write-only" | "read-write") {
            return Err(
                format!("full-register write {name} requires a writable 32-bit register").into(),
            );
        }
        let fields = register
            .children()
            .find(|node| node.has_tag_name("fields"))
            .ok_or_else(|| format!("full-register write {name} register has no fields"))?;
        if fields
            .children()
            .filter(|node| node.has_tag_name("field"))
            .count()
            != 1
        {
            return Err(format!(
                "full-register write {name} register must contain exactly one field"
            )
            .into());
        }
        let field = direct_named_child(fields, "field", field_name).ok_or_else(|| {
            format!("full-register write {name} references unknown field {field_name}")
        })?;
        if child_u64(field, "bitOffset")? != 0 || child_u64(field, "bitWidth")? != 32 {
            return Err(format!(
                "full-register write {name} field must cover the complete 32-bit register"
            )
            .into());
        }
        let constraint = field
            .children()
            .find(|node| node.has_tag_name("writeConstraint"))
            .and_then(|node| node.children().find(|child| child.has_tag_name("range")))
            .ok_or_else(|| format!("full-register write {name} has no range constraint"))?;
        if parse_u64(child_text(constraint, "minimum")?, "write minimum")? != 0
            || parse_u64(child_text(constraint, "maximum")?, "write maximum")? != u32::MAX.into()
        {
            return Err(
                format!("full-register write {name} field must accept every 32-bit value").into(),
            );
        }

        bindings.push(FullRegisterWriteBinding {
            name: name.to_owned(),
            peripheral: peripheral_name.to_owned(),
            register: register_name.to_owned(),
            field: field_name.to_owned(),
        });
    }
    Ok(bindings)
}

fn generate_full_register_write_api(document: &Document<'_>) -> Result<String, Box<dyn Error>> {
    let bindings = parse_full_register_writes(document)?;
    if bindings.is_empty() {
        return Ok(String::new());
    }

    let mut output = String::from(
        "\n/// Safe, SVD-declared writes which cover a complete register.\n\
         pub mod full_register_write {\n",
    );
    for binding in bindings {
        let peripheral_type = type_binding_name(&binding.peripheral);
        let register = member_binding_name(&binding.register);
        let field = member_binding_name(&binding.field);
        output.push_str(&format!(
            "\n    /// Write every bit of `{}`.`{}` through its full-width field.\n\
             #[inline]\n\
             pub fn {}(registers: &crate::{peripheral_type}, value: u32) {{\n\
                 // SAFETY: generator validation proves that this is the only field,\n\
                 // it covers all 32 bits and accepts every `u32`; no zero-filled\n\
                 // reserved or partially described bits remain.\n\
                 unsafe {{\n\
                     registers.{register}().write_with_zero(|writer|\n\
                         writer.{field}().set(value)\n\
                     );\n\
                 }}\n\
             }}\n",
            binding.peripheral, binding.register, binding.name,
        ));
    }
    output.push_str("}\n");
    Ok(output)
}

fn parse_fixed_register_writes(
    document: &Document<'_>,
) -> Result<Vec<FixedRegisterWriteBinding>, Box<dyn Error>> {
    let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioFixedRegisterWrites"))
    else {
        return Ok(Vec::new());
    };
    if document
        .descendants()
        .filter(|node| node.has_tag_name("openEspRadioFixedRegisterWrites"))
        .count()
        != 1
    {
        return Err("SVD has duplicate openEspRadioFixedRegisterWrites extensions".into());
    }

    let peripherals = document
        .descendants()
        .find(|node| node.has_tag_name("peripherals"))
        .ok_or("SVD has no peripherals element")?;
    let mut names = BTreeSet::new();
    let mut bindings = Vec::new();
    for write in extension
        .children()
        .filter(|node| node.has_tag_name("write"))
    {
        let name = required_attribute(write, "name")?;
        if name.is_empty()
            || member_binding_name(name) != name
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
            || !name.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(
                format!("fixed-register write name {name:?} is not lower snake case").into(),
            );
        }
        if !names.insert(name) {
            return Err(format!("duplicate fixed-register write name {name}").into());
        }

        let peripheral_name = required_attribute(write, "peripheral")?;
        let register_name = required_attribute(write, "register")?;
        let field_name = required_attribute(write, "field")?;
        let variant_name = required_attribute(write, "variant")?;
        required_attribute(write, "source")?;
        let peripheral = direct_named_child(peripherals, "peripheral", peripheral_name)
            .ok_or_else(|| {
                format!(
                    "fixed-register write {name} references unknown peripheral {peripheral_name}"
                )
            })?;
        let registers = peripheral
            .children()
            .find(|node| node.has_tag_name("registers"))
            .ok_or_else(|| format!("peripheral {peripheral_name} has no registers"))?;
        let register =
            direct_named_child(registers, "register", register_name).ok_or_else(|| {
                format!("fixed-register write {name} references unknown register {register_name}")
            })?;
        let access = child_text(register, "access")?;
        if child_u64(register, "size")? != 32 || !matches!(access, "write-only" | "read-write") {
            return Err(
                format!("fixed-register write {name} requires a writable 32-bit register").into(),
            );
        }
        let fields = register
            .children()
            .find(|node| node.has_tag_name("fields"))
            .ok_or_else(|| format!("fixed-register write {name} register has no fields"))?;
        if fields
            .children()
            .filter(|node| node.has_tag_name("field"))
            .count()
            != 1
        {
            return Err(format!(
                "fixed-register write {name} register must contain exactly one field"
            )
            .into());
        }
        let field = direct_named_child(fields, "field", field_name).ok_or_else(|| {
            format!("fixed-register write {name} references unknown field {field_name}")
        })?;
        if child_u64(field, "bitOffset")? != 0 || child_u64(field, "bitWidth")? != 32 {
            return Err(format!(
                "fixed-register write {name} field must cover the complete 32-bit register"
            )
            .into());
        }
        let has_variant = field
            .children()
            .filter(|node| node.has_tag_name("enumeratedValues"))
            .filter(|values| optional_child_text(*values, "usage") != Some("read"))
            .flat_map(|values| {
                values
                    .children()
                    .filter(|node| node.has_tag_name("enumeratedValue"))
            })
            .any(|variant| optional_child_text(variant, "name") == Some(variant_name));
        if !has_variant {
            return Err(format!(
                "fixed-register write {name} references unknown writable variant {variant_name}"
            )
            .into());
        }

        bindings.push(FixedRegisterWriteBinding {
            name: name.to_owned(),
            peripheral: peripheral_name.to_owned(),
            register: register_name.to_owned(),
            field: field_name.to_owned(),
            variant: variant_name.to_owned(),
        });
    }
    Ok(bindings)
}

fn generate_fixed_register_write_api(document: &Document<'_>) -> Result<String, Box<dyn Error>> {
    let bindings = parse_fixed_register_writes(document)?;
    if bindings.is_empty() {
        return Ok(String::new());
    }
    let mut output = String::from(
        "\n/// Safe, SVD-declared complete-register writes of fixed enumerated values.\n\
         pub mod fixed_register_write {\n",
    );
    for binding in bindings {
        let peripheral_type = type_binding_name(&binding.peripheral);
        let register = member_binding_name(&binding.register);
        let field = member_binding_name(&binding.field);
        let variant = member_binding_name(&binding.variant);
        output.push_str(&format!(
            "\n    /// Write the `{}` variant to every bit of `{}`.`{}`.\n\
             #[inline]\n\
             pub fn {}(registers: &crate::{peripheral_type}) {{\n\
                 // SAFETY: generator validation proves that the sole field covers\n\
                 // all 32 bits and the named writable variant exists in the SVD.\n\
                 unsafe {{\n\
                     registers.{register}().write_with_zero(|writer|\n\
                         writer.{field}().{variant}()\n\
                     );\n\
                 }}\n\
             }}\n",
            binding.variant, binding.peripheral, binding.register, binding.name,
        ));
    }
    output.push_str("}\n");
    Ok(output)
}

fn parse_fixed_register_images(
    document: &Document<'_>,
) -> Result<Vec<FixedRegisterImageBinding>, Box<dyn Error>> {
    let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioFixedRegisterImages"))
    else {
        return Ok(Vec::new());
    };
    if document
        .descendants()
        .filter(|node| node.has_tag_name("openEspRadioFixedRegisterImages"))
        .count()
        != 1
    {
        return Err("SVD has duplicate openEspRadioFixedRegisterImages extensions".into());
    }

    let peripherals = document
        .descendants()
        .find(|node| node.has_tag_name("peripherals"))
        .ok_or("SVD has no peripherals element")?;
    let mut names = BTreeSet::new();
    let mut bindings = Vec::new();
    for write in extension
        .children()
        .filter(|node| node.has_tag_name("write"))
    {
        let name = required_attribute(write, "name")?;
        if name.is_empty()
            || member_binding_name(name) != name
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
            || !name.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(
                format!("fixed-register image name {name:?} is not lower snake case").into(),
            );
        }
        if !names.insert(name) {
            return Err(format!("duplicate fixed-register image name {name}").into());
        }

        let peripheral_name = required_attribute(write, "peripheral")?;
        let register_name = required_attribute(write, "register")?;
        let value = parse_u64(required_attribute(write, "value")?, "fixed register image")?;
        let value = u32::try_from(value)
            .map_err(|_| format!("fixed-register image {name} does not fit 32 bits"))?;
        required_attribute(write, "source")?;
        let peripheral = direct_named_child(peripherals, "peripheral", peripheral_name)
            .ok_or_else(|| {
                format!(
                    "fixed-register image {name} references unknown peripheral {peripheral_name}"
                )
            })?;
        let registers = peripheral
            .children()
            .find(|node| node.has_tag_name("registers"))
            .ok_or_else(|| format!("peripheral {peripheral_name} has no registers"))?;
        let register =
            direct_named_child(registers, "register", register_name).ok_or_else(|| {
                format!("fixed-register image {name} references unknown register {register_name}")
            })?;
        let access = child_text(register, "access")?;
        if child_u64(register, "size")? != 32 || !matches!(access, "write-only" | "read-write") {
            return Err(
                format!("fixed-register image {name} requires a writable 32-bit register").into(),
            );
        }
        if register
            .children()
            .any(|node| node.has_tag_name("modifiedWriteValues"))
        {
            return Err(format!(
                "fixed-register image {name} cannot target modified-write semantics"
            )
            .into());
        }

        bindings.push(FixedRegisterImageBinding {
            name: name.to_owned(),
            peripheral: peripheral_name.to_owned(),
            register: register_name.to_owned(),
            value,
            register_is_array: register.children().any(|node| node.has_tag_name("dim")),
        });
    }
    Ok(bindings)
}

fn generate_fixed_register_image_api(document: &Document<'_>) -> Result<String, Box<dyn Error>> {
    let bindings = parse_fixed_register_images(document)?;
    if bindings.is_empty() {
        return Ok(String::new());
    }
    let mut output = String::from(
        "\n/// Safe, SVD-declared writes of fixed complete-register images.\n\
         pub mod fixed_register_image {\n",
    );
    for binding in bindings {
        let peripheral_type = type_binding_name(&binding.peripheral);
        let register = member_binding_name(&binding.register);
        let (index_parameter, index_argument) = if binding.register_is_array {
            (", index: usize", "index")
        } else {
            ("", "")
        };
        output.push_str(&format!(
            "\n    /// Publish the SVD-qualified image `0x{:08x}` to `{}`.`{}`.\n\
             #[inline]\n\
             pub fn {}(registers: &crate::{peripheral_type}{index_parameter}) {{\n\
                 // SAFETY: generator validation proves that the target is an\n\
                 // ordinary writable 32-bit register, while the SVD extension\n\
                 // and its provenance qualify this exact complete image.\n\
                 unsafe {{\n\
                     registers.{register}({index_argument}).write_with_zero(|writer|\n\
                         writer.bits(0x{:08x})\n\
                     );\n\
                 }}\n\
             }}\n",
            binding.value, binding.peripheral, binding.register, binding.name, binding.value,
        ));
    }
    output.push_str("}\n");
    Ok(output)
}

fn parse_register_image_writes(
    document: &Document<'_>,
) -> Result<Vec<RegisterImageWriteBinding>, Box<dyn Error>> {
    let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioRegisterImageWrites"))
    else {
        return Ok(Vec::new());
    };
    if document
        .descendants()
        .filter(|node| node.has_tag_name("openEspRadioRegisterImageWrites"))
        .count()
        != 1
    {
        return Err("SVD has duplicate openEspRadioRegisterImageWrites extensions".into());
    }

    let peripherals = document
        .descendants()
        .find(|node| node.has_tag_name("peripherals"))
        .ok_or("SVD has no peripherals element")?;
    let mut names = BTreeSet::new();
    let mut bindings = Vec::new();
    for write in extension
        .children()
        .filter(|node| node.has_tag_name("write"))
    {
        let name = required_attribute(write, "name")?;
        if name.is_empty()
            || member_binding_name(name) != name
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
            || !name.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(
                format!("register-image write name {name:?} is not lower snake case").into(),
            );
        }
        if !names.insert(name) {
            return Err(format!("duplicate register-image write name {name}").into());
        }

        let peripheral_name = required_attribute(write, "peripheral")?;
        let register_name = required_attribute(write, "register")?;
        required_attribute(write, "source")?;
        let peripheral = direct_named_child(peripherals, "peripheral", peripheral_name)
            .ok_or_else(|| {
                format!(
                    "register-image write {name} references unknown peripheral {peripheral_name}"
                )
            })?;
        let registers = peripheral
            .children()
            .find(|node| node.has_tag_name("registers"))
            .ok_or_else(|| format!("peripheral {peripheral_name} has no registers"))?;
        let register =
            direct_named_child(registers, "register", register_name).ok_or_else(|| {
                format!("register-image write {name} references unknown register {register_name}")
            })?;
        let access = child_text(register, "access")?;
        if child_u64(register, "size")? != 32 || !matches!(access, "write-only" | "read-write") {
            return Err(
                format!("register-image write {name} requires a writable 32-bit register").into(),
            );
        }
        if register
            .children()
            .any(|node| node.has_tag_name("modifiedWriteValues"))
        {
            return Err(format!(
                "register-image write {name} cannot target modified-write semantics"
            )
            .into());
        }

        bindings.push(RegisterImageWriteBinding {
            name: name.to_owned(),
            peripheral: peripheral_name.to_owned(),
            register: register_name.to_owned(),
            register_is_array: register.children().any(|node| node.has_tag_name("dim")),
        });
    }
    Ok(bindings)
}

fn generate_register_image_write_api(document: &Document<'_>) -> Result<String, Box<dyn Error>> {
    let bindings = parse_register_image_writes(document)?;
    if bindings.is_empty() {
        return Ok(String::new());
    }
    let mut output = String::from(
        "\n/// Safe, SVD-declared writes of dynamic complete-register images.\n\
         pub mod register_image_write {\n",
    );
    for binding in bindings {
        let peripheral_type = type_binding_name(&binding.peripheral);
        let register = member_binding_name(&binding.register);
        let (index_parameter, index_argument) = if binding.register_is_array {
            ("index: usize, ", "index")
        } else {
            ("", "")
        };
        output.push_str(&format!(
            "\n    /// Publish a caller-built complete image to `{}`.`{}`.\n\
             #[inline]\n\
             pub fn {}(registers: &crate::{peripheral_type}, {index_parameter}image: u32) {{\n\
                 // SAFETY: generator validation proves that the target is an\n\
                 // ordinary writable 32-bit register. The SVD extension and\n\
                 // its provenance qualify this semantic whole-image operation.\n\
                 unsafe {{\n\
                     registers.{register}({index_argument}).write_with_zero(|writer|\n\
                         writer.bits(image)\n\
                     );\n\
                 }}\n\
             }}\n",
            binding.peripheral, binding.register, binding.name,
        ));
    }
    output.push_str("}\n");
    Ok(output)
}

fn parse_zero_based_field_writes(
    document: &Document<'_>,
) -> Result<Vec<ZeroBasedFieldWriteBinding>, Box<dyn Error>> {
    let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioZeroBasedFieldWrites"))
    else {
        return Ok(Vec::new());
    };
    if document
        .descendants()
        .filter(|node| node.has_tag_name("openEspRadioZeroBasedFieldWrites"))
        .count()
        != 1
    {
        return Err("SVD has duplicate openEspRadioZeroBasedFieldWrites extensions".into());
    }

    let peripherals = document
        .descendants()
        .find(|node| node.has_tag_name("peripherals"))
        .ok_or("SVD has no peripherals element")?;
    let mut names = BTreeSet::new();
    let mut bindings = Vec::new();
    for write in extension
        .children()
        .filter(|node| node.has_tag_name("write"))
    {
        let name = required_attribute(write, "name")?;
        if name.is_empty()
            || member_binding_name(name) != name
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
            || !name.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(
                format!("zero-based field write name {name:?} is not lower snake case").into(),
            );
        }
        if !names.insert(name) {
            return Err(format!("duplicate zero-based field write name {name}").into());
        }

        let peripheral_name = required_attribute(write, "peripheral")?;
        let register_name = required_attribute(write, "register")?;
        let field_names = match (write.attribute("field"), write.attribute("fields")) {
            (Some(field), None) => vec![field],
            (None, Some(fields)) => {
                let names = fields.split(',').map(str::trim).collect::<Vec<_>>();
                if names.is_empty() || names.iter().any(|field| field.is_empty()) {
                    return Err(
                        format!("zero-based field write {name} has an empty fields list").into(),
                    );
                }
                names
            }
            (Some(_), Some(_)) => {
                return Err(format!(
                    "zero-based field write {name} must use either field or fields, not both"
                )
                .into());
            }
            (None, None) => {
                return Err(format!(
                    "zero-based field write {name} requires a field or fields attribute"
                )
                .into());
            }
        };
        required_attribute(write, "source")?;
        let peripheral = direct_named_child(peripherals, "peripheral", peripheral_name)
            .ok_or_else(|| {
                format!(
                    "zero-based field write {name} references unknown peripheral {peripheral_name}"
                )
            })?;
        let registers = peripheral
            .children()
            .find(|node| node.has_tag_name("registers"))
            .ok_or_else(|| format!("peripheral {peripheral_name} has no registers"))?;
        let register =
            direct_named_child(registers, "register", register_name).ok_or_else(|| {
                format!("zero-based field write {name} references unknown register {register_name}")
            })?;
        let access = child_text(register, "access")?;
        if child_u64(register, "size")? != 32 || !matches!(access, "write-only" | "read-write") {
            return Err(format!(
                "zero-based field write {name} requires a writable 32-bit register"
            )
            .into());
        }
        let fields = register
            .children()
            .find(|node| node.has_tag_name("fields"))
            .ok_or_else(|| format!("zero-based field write {name} register has no fields"))?;
        let mut selected_names = BTreeSet::new();
        let mut selected_fields = Vec::new();
        for field_name in field_names {
            if !selected_names.insert(field_name) {
                return Err(
                    format!("zero-based field write {name} repeats field {field_name}").into(),
                );
            }
            let field = direct_named_child(fields, "field", field_name).ok_or_else(|| {
                format!("zero-based field write {name} references unknown field {field_name}")
            })?;
            if optional_child_text(field, "access") == Some("read-only") {
                return Err(format!(
                    "zero-based field write {name} field {field_name} is read-only"
                )
                .into());
            }
            let width = parse_u64(child_text(field, "bitWidth")?, "field width")?;
            if !(1..=32).contains(&width) {
                return Err(format!(
                    "zero-based field write {name} requires field {field_name} to be between 1 and 32 bits"
                )
                .into());
            }
            if width != 1 {
                let constraint = field
                    .children()
                    .find(|node| node.has_tag_name("writeConstraint"))
                    .and_then(|node| node.children().find(|child| child.has_tag_name("range")))
                    .ok_or_else(|| {
                        format!(
                            "zero-based field write {name} field {field_name} has no range constraint"
                        )
                    })?;
                let maximum = if width == 32 {
                    u64::from(u32::MAX)
                } else {
                    (1_u64 << width) - 1
                };
                if parse_u64(child_text(constraint, "minimum")?, "write minimum")? != 0
                    || parse_u64(child_text(constraint, "maximum")?, "write maximum")? != maximum
                {
                    return Err(format!(
                        "zero-based field write {name} field {field_name} must accept every representable value"
                    )
                    .into());
                }
            }
            let value_type = match width {
                1 => "bool",
                2..=8 => "u8",
                9..=16 => "u16",
                17..=32 => "u32",
                _ => unreachable!(),
            };
            selected_fields.push(ZeroBasedFieldBinding {
                name: field_name.to_owned(),
                value_type,
            });
        }

        bindings.push(ZeroBasedFieldWriteBinding {
            name: name.to_owned(),
            peripheral: peripheral_name.to_owned(),
            register: register_name.to_owned(),
            fields: selected_fields,
            register_is_array: register.children().any(|node| node.has_tag_name("dim")),
        });
    }
    Ok(bindings)
}

fn generate_zero_based_field_write_api(document: &Document<'_>) -> Result<String, Box<dyn Error>> {
    let bindings = parse_zero_based_field_writes(document)?;
    if bindings.is_empty() {
        return Ok(String::new());
    }

    let mut output = String::from(
        "\n/// Safe, SVD-declared field writes based on an all-zero register image.\n\
         pub mod zero_based_field_write {\n",
    );
    for binding in bindings {
        let peripheral_type = type_binding_name(&binding.peripheral);
        let register = member_binding_name(&binding.register);
        let (index_parameter, index_argument) = if binding.register_is_array {
            ("index: usize, ", "index")
        } else {
            ("", "")
        };
        let field_list = binding
            .fields
            .iter()
            .map(|field| format!("`{}`", field.name))
            .collect::<Vec<_>>()
            .join(", ");
        let (value_parameters, field_writes) = if binding.fields.len() == 1 {
            let field = &binding.fields[0];
            let field_name = member_binding_name(&field.name);
            let write = if field.value_type == "bool" {
                format!("writer.{field_name}().bit(value)")
            } else {
                format!("writer.{field_name}().set(value)")
            };
            (format!("value: {}", field.value_type), write)
        } else {
            let parameters = binding
                .fields
                .iter()
                .map(|field| {
                    format!(
                        "{}_value: {}",
                        member_binding_name(&field.name),
                        field.value_type
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let writes = binding
                .fields
                .iter()
                .map(|field| {
                    let field_name = member_binding_name(&field.name);
                    let method = if field.value_type == "bool" {
                        "bit"
                    } else {
                        "set"
                    };
                    format!(".{field_name}().{method}({field_name}_value)")
                })
                .collect::<String>();
            (parameters, format!("writer{writes}"))
        };
        output.push_str(&format!(
            "\n    /// Write {field_list} in `{}`.`{}` while publishing zero to every other register bit.\n\
             #[inline]\n\
             pub fn {}(registers: &crate::{peripheral_type}, {index_parameter}{value_parameters}) {{\n\
                 // SAFETY: the SVD extension explicitly qualifies the zero-based\n\
                 // transaction, and generator validation proves every selected field\n\
                 // accepts every value representable by its public argument type.\n\
                 unsafe {{\n\
                     registers.{register}({index_argument}).write_with_zero(|writer|\n\
                         {field_writes}\n\
                     );\n\
                 }}\n\
             }}\n",
            binding.peripheral, binding.register, binding.name,
        ));
    }
    output.push_str("}\n");
    Ok(output)
}

fn parse_zero_register_writes(
    document: &Document<'_>,
) -> Result<Vec<ZeroRegisterWriteBinding>, Box<dyn Error>> {
    let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioZeroRegisterWrites"))
    else {
        return Ok(Vec::new());
    };
    if document
        .descendants()
        .filter(|node| node.has_tag_name("openEspRadioZeroRegisterWrites"))
        .count()
        != 1
    {
        return Err("SVD has duplicate openEspRadioZeroRegisterWrites extensions".into());
    }

    let peripherals = document
        .descendants()
        .find(|node| node.has_tag_name("peripherals"))
        .ok_or("SVD has no peripherals element")?;
    let mut names = BTreeSet::new();
    let mut bindings = Vec::new();
    for write in extension
        .children()
        .filter(|node| node.has_tag_name("write"))
    {
        let name = required_attribute(write, "name")?;
        if name.is_empty()
            || member_binding_name(name) != name
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
            || !name.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(
                format!("zero-register write name {name:?} is not lower snake case").into(),
            );
        }
        if !names.insert(name) {
            return Err(format!("duplicate zero-register write name {name}").into());
        }

        let peripheral_name = required_attribute(write, "peripheral")?;
        let register_name = required_attribute(write, "register")?;
        required_attribute(write, "source")?;
        let peripheral = direct_named_child(peripherals, "peripheral", peripheral_name)
            .ok_or_else(|| {
                format!(
                    "zero-register write {name} references unknown peripheral {peripheral_name}"
                )
            })?;
        let registers = peripheral
            .children()
            .find(|node| node.has_tag_name("registers"))
            .ok_or_else(|| format!("peripheral {peripheral_name} has no registers"))?;
        let register =
            direct_named_child(registers, "register", register_name).ok_or_else(|| {
                format!("zero-register write {name} references unknown register {register_name}")
            })?;
        let access = child_text(register, "access")?;
        if child_u64(register, "size")? != 32 || !matches!(access, "write-only" | "read-write") {
            return Err(
                format!("zero-register write {name} requires a writable 32-bit register").into(),
            );
        }
        if register
            .children()
            .any(|node| node.has_tag_name("modifiedWriteValues"))
        {
            return Err(format!(
                "zero-register write {name} cannot target modified-write semantics"
            )
            .into());
        }

        bindings.push(ZeroRegisterWriteBinding {
            name: name.to_owned(),
            peripheral: peripheral_name.to_owned(),
            register: register_name.to_owned(),
            register_is_array: register.children().any(|node| node.has_tag_name("dim")),
        });
    }
    Ok(bindings)
}

fn generate_zero_register_write_api(document: &Document<'_>) -> Result<String, Box<dyn Error>> {
    let bindings = parse_zero_register_writes(document)?;
    if bindings.is_empty() {
        return Ok(String::new());
    }

    let mut output = String::from(
        "\n/// Safe, SVD-declared complete-register zero writes.\n\
         pub mod zero_register_write {\n",
    );
    for binding in bindings {
        let peripheral_type = type_binding_name(&binding.peripheral);
        let register = member_binding_name(&binding.register);
        let (index_parameter, index_argument) = if binding.register_is_array {
            (", index: usize", "index")
        } else {
            ("", "")
        };
        output.push_str(&format!(
            "\n    /// Publish zero to every bit of `{}`.`{}`.\n\
             #[inline]\n\
             pub fn {}(registers: &crate::{peripheral_type}{index_parameter}) {{\n\
                 // SAFETY: the SVD extension and its provenance explicitly\n\
                 // qualify a complete zero write to this ordinary register.\n\
                 unsafe {{\n\
                     registers.{register}({index_argument}).write_with_zero(|writer| writer);\n\
                 }}\n\
             }}\n",
            binding.peripheral, binding.register, binding.name,
        ));
    }
    output.push_str("}\n");
    Ok(output)
}

fn parse_masked_register_modifies(
    document: &Document<'_>,
) -> Result<Vec<MaskedRegisterModifyBinding>, Box<dyn Error>> {
    let Some(extension) = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioMaskedRegisterModifies"))
    else {
        return Ok(Vec::new());
    };
    if document
        .descendants()
        .filter(|node| node.has_tag_name("openEspRadioMaskedRegisterModifies"))
        .count()
        != 1
    {
        return Err("SVD has duplicate openEspRadioMaskedRegisterModifies extensions".into());
    }

    let peripherals = document
        .descendants()
        .find(|node| node.has_tag_name("peripherals"))
        .ok_or("SVD has no peripherals element")?;
    let mut names = BTreeSet::new();
    let mut bindings = Vec::new();
    for modify in extension
        .children()
        .filter(|node| node.has_tag_name("modify"))
    {
        let name = required_attribute(modify, "name")?;
        if name.is_empty()
            || member_binding_name(name) != name
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
            || !name.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(
                format!("masked-register modify name {name:?} is not lower snake case").into(),
            );
        }
        if !names.insert(name) {
            return Err(format!("duplicate masked-register modify name {name}").into());
        }

        let peripheral_name = required_attribute(modify, "peripheral")?;
        let register_name = required_attribute(modify, "register")?;
        required_attribute(modify, "source")?;
        let parse_mask = |attribute| -> Result<u32, Box<dyn Error>> {
            let value = parse_u64(required_attribute(modify, attribute)?, attribute)?;
            value
                .try_into()
                .map_err(|_| format!("{attribute} for {name} exceeds 32 bits").into())
        };
        let preserve_mask = parse_mask("preserveMask")?;
        let input_mask = parse_mask("inputMask")?;
        let set_mask = parse_mask("setMask")?;
        if preserve_mask & input_mask != 0
            || preserve_mask & set_mask != 0
            || input_mask & set_mask != 0
        {
            return Err(format!("masked-register modify {name} masks overlap").into());
        }
        if preserve_mask | input_mask | set_mask != u32::MAX {
            return Err(format!(
                "masked-register modify {name} masks must partition all 32 register bits"
            )
            .into());
        }

        let peripheral = direct_named_child(peripherals, "peripheral", peripheral_name)
            .ok_or_else(|| {
                format!(
                    "masked-register modify {name} references unknown peripheral {peripheral_name}"
                )
            })?;
        let registers = peripheral
            .children()
            .find(|node| node.has_tag_name("registers"))
            .ok_or_else(|| format!("peripheral {peripheral_name} has no registers"))?;
        let register =
            direct_named_child(registers, "register", register_name).ok_or_else(|| {
                format!("masked-register modify {name} references unknown register {register_name}")
            })?;
        if child_u64(register, "size")? != 32 || child_text(register, "access")? != "read-write" {
            return Err(format!(
                "masked-register modify {name} requires a read-write 32-bit register"
            )
            .into());
        }
        if register
            .descendants()
            .any(|node| node.has_tag_name("modifiedWriteValues"))
        {
            return Err(format!(
                "masked-register modify {name} cannot target modified-write semantics"
            )
            .into());
        }

        bindings.push(MaskedRegisterModifyBinding {
            name: name.to_owned(),
            peripheral: peripheral_name.to_owned(),
            register: register_name.to_owned(),
            preserve_mask,
            input_mask,
            set_mask,
            register_is_array: register.children().any(|node| node.has_tag_name("dim")),
        });
    }
    Ok(bindings)
}

fn generate_masked_register_modify_api(document: &Document<'_>) -> Result<String, Box<dyn Error>> {
    let bindings = parse_masked_register_modifies(document)?;
    if bindings.is_empty() {
        return Ok(String::new());
    }

    let mut output = String::from(
        "\n/// Safe, SVD-declared masked read-modify-write transactions.\n\
         pub mod masked_register_modify {\n",
    );
    for binding in bindings {
        let peripheral_type = type_binding_name(&binding.peripheral);
        let register = member_binding_name(&binding.register);
        let (index_parameter, index_argument) = if binding.register_is_array {
            ("index: usize, ", "index")
        } else {
            ("", "")
        };
        output.push_str(&format!(
            "\n    /// Preserve mask 0x{:08x}, accept input mask 0x{:08x}, and set 0x{:08x} in {}.{}.\n\
             #[inline]\n\
             pub fn {}(registers: &crate::{peripheral_type}, {index_parameter}input: u32) {{\n\
                 registers.{register}({index_argument}).modify(|reader, writer| {{\n\
                     let image = (reader.bits() & 0x{:08x})\n\
                         | (input & 0x{:08x})\n\
                         | 0x{:08x};\n\
                     // SAFETY: generator validation proves the three masks are\n\
                     // disjoint and partition every bit of this ordinary register.\n\
                     unsafe {{ writer.bits(image) }}\n\
                 }});\n\
             }}\n",
            binding.preserve_mask,
            binding.input_mask,
            binding.set_mask,
            binding.peripheral,
            binding.register,
            binding.name,
            binding.preserve_mask,
            binding.input_mask,
            binding.set_mask,
        ));
    }
    output.push_str("}\n");
    Ok(output)
}

fn expanded_register_addresses(
    peripheral_name: &str,
    peripheral_base: u64,
    parent_offset: u64,
    scope: &[ExpandedScope],
    children: &[RegisterCluster],
    inherited: RegisterProperties,
    addresses: &mut BTreeMap<u64, Vec<ExpandedRegister>>,
) -> Result<(), Box<dyn Error>> {
    for child in children {
        match child {
            RegisterCluster::Register(register) => {
                let instances = match register {
                    MaybeArray::Single(info) => {
                        vec![(info.clone(), member_binding_name(&info.name), None)]
                    }
                    MaybeArray::Array(info, dim) => svd_parser::svd::register::expand(info, dim)
                        .enumerate()
                        .map(|(index, expanded)| {
                            (
                                expanded,
                                array_binding_name(&info.name, dim.dim_name.as_deref()),
                                Some(index as u32),
                            )
                        })
                        .collect(),
                };
                for (info, rust_name, array_index) in instances {
                    let properties = inherited_properties(inherited, info.properties);
                    let size_bits = properties
                        .size
                        .ok_or_else(|| format!("register {} has no inherited size", info.name))?;
                    let mut expanded_fields = Vec::new();
                    if let Some(fields) = &info.fields {
                        let mut names = BTreeSet::new();
                        let mut occupied = 0_u128;
                        for field in fields {
                            let instances = match field {
                                MaybeArray::Single(info) => {
                                    vec![(info.clone(), member_binding_name(&info.name), None)]
                                }
                                MaybeArray::Array(info, dim) => {
                                    svd_parser::svd::field::expand(info, dim)
                                        .enumerate()
                                        .map(|(index, expanded)| {
                                            (
                                                expanded,
                                                array_binding_name(
                                                    &info.name,
                                                    dim.dim_name.as_deref(),
                                                ),
                                                Some(index as u32),
                                            )
                                        })
                                        .collect()
                                }
                            };
                            for (field, rust_name, array_index) in instances {
                                if !names.insert(field.name.clone()) {
                                    return Err(format!(
                                        "register {} contains duplicate expanded field name {}",
                                        info.name, field.name
                                    )
                                    .into());
                                }
                                let offset = field.bit_offset();
                                let width = field.bit_width();
                                if width == 0
                                    || offset.checked_add(width).is_none_or(|end| end > size_bits)
                                {
                                    return Err(format!(
                                        "register {} field {} has invalid expanded bit range {offset}+{width} for a {size_bits}-bit register",
                                        info.name, field.name
                                    )
                                    .into());
                                }
                                let mask = if width == 128 {
                                    u128::MAX
                                } else {
                                    ((1_u128 << width) - 1) << offset
                                };
                                if occupied & mask != 0 {
                                    return Err(format!(
                                        "register {} field {} overlaps another expanded field",
                                        info.name, field.name
                                    )
                                    .into());
                                }
                                occupied |= mask;
                                expanded_fields.push(ExpandedField {
                                    name: field.name,
                                    rust_name,
                                    array_index,
                                    bit_offset: offset,
                                    bit_width: width,
                                    access: field.access.or(properties.access),
                                });
                            }
                        }
                    }
                    expanded_fields.sort_by(|left, right| {
                        (left.bit_offset, &left.name).cmp(&(right.bit_offset, &right.name))
                    });
                    let address = peripheral_base
                        .checked_add(parent_offset)
                        .and_then(|base| base.checked_add(u64::from(info.address_offset)))
                        .ok_or("SVD register address overflow")?;
                    let mut identity = peripheral_name.to_owned();
                    for item in scope {
                        identity.push('.');
                        identity.push_str(&item.identity_name);
                    }
                    identity.push('.');
                    identity.push_str(&info.name);
                    addresses
                        .entry(address)
                        .or_default()
                        .push(ExpandedRegister {
                            identity,
                            peripheral: peripheral_name.to_owned(),
                            scope: scope.to_vec(),
                            name: info.name.clone(),
                            rust_name,
                            array_index,
                            size_bits,
                            access: properties.access,
                            alternate_group: info.alternate_group.clone(),
                            alternate_register: info.alternate_register.clone(),
                            fields: expanded_fields,
                        });
                }
            }
            RegisterCluster::Cluster(cluster) => {
                let instances = match cluster {
                    MaybeArray::Single(info) => {
                        vec![(info.clone(), member_binding_name(&info.name), None)]
                    }
                    MaybeArray::Array(info, dim) => svd_parser::svd::cluster::expand(info, dim)
                        .enumerate()
                        .map(|(index, expanded)| {
                            (
                                expanded,
                                array_binding_name(&info.name, dim.dim_name.as_deref()),
                                Some(index as u32),
                            )
                        })
                        .collect(),
                };
                for (info, rust_name, array_index) in instances {
                    let properties =
                        inherited_properties(inherited, info.default_register_properties);
                    let mut child_scope = scope.to_vec();
                    child_scope.push(ExpandedScope {
                        identity_name: info.name.clone(),
                        rust_name,
                        array_index,
                    });
                    expanded_register_addresses(
                        peripheral_name,
                        peripheral_base,
                        parent_offset + u64::from(info.address_offset),
                        &child_scope,
                        &info.children,
                        properties,
                        addresses,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn explicitly_alternate(registers: &[ExpandedRegister]) -> bool {
    if registers.len() < 2 {
        return false;
    }
    let first = &registers[0];
    if registers
        .iter()
        .any(|register| register.peripheral != first.peripheral || register.scope != first.scope)
    {
        return false;
    }
    let same_group = first.alternate_group.as_ref().is_some_and(|group| {
        registers
            .iter()
            .all(|register| register.alternate_group.as_ref() == Some(group))
    });
    let direct_reference = registers.iter().any(|canonical| {
        canonical.alternate_register.is_none()
            && registers.iter().all(|register| {
                register.name == canonical.name
                    || register.alternate_register.as_deref() == Some(canonical.name.as_str())
            })
    });
    same_group || direct_reference
}

fn validate_alias_group(
    address: u64,
    registers: &[ExpandedRegister],
) -> Result<(), Box<dyn Error>> {
    if registers.len() == 1 {
        if registers[0].alternate_group.is_some() || registers[0].alternate_register.is_some() {
            return Err(format!(
                "register {} declares an alternate view without another register at 0x{address:08x}",
                registers[0].identity
            )
            .into());
        }
        return Ok(());
    }
    if !explicitly_alternate(registers) {
        return Err(format!(
            "physical register address 0x{address:08x} has unmarked aliases: {registers:?}"
        )
        .into());
    }
    let canonical = &registers[0];
    if registers
        .iter()
        .any(|register| register.size_bits != canonical.size_bits)
    {
        return Err(
            format!("register aliases at 0x{address:08x} disagree on size: {registers:?}").into(),
        );
    }
    if registers
        .iter()
        .any(|register| register.access != canonical.access)
    {
        return Err(format!(
            "register aliases at 0x{address:08x} disagree on access: {registers:?}"
        )
        .into());
    }
    Ok(())
}

fn expanded_register_map(
    input: &str,
) -> Result<BTreeMap<u64, Vec<ExpandedRegister>>, Box<dyn Error>> {
    let device = svd_parser::parse(input)?;
    let mut addresses = BTreeMap::new();
    for peripheral in &device.peripherals {
        let instances = match peripheral {
            MaybeArray::Single(info) => vec![info.clone()],
            MaybeArray::Array(info, dim) => {
                svd_parser::svd::peripheral::expand(info, dim).collect()
            }
        };
        let validate_instance = |peripheral: &svd_parser::svd::PeripheralInfo,
                                 addresses: &mut BTreeMap<u64, Vec<ExpandedRegister>>|
         -> Result<(), Box<dyn Error>> {
            let properties = inherited_properties(
                device.default_register_properties,
                peripheral.default_register_properties,
            );
            if let Some(registers) = &peripheral.registers {
                expanded_register_addresses(
                    &peripheral.name,
                    peripheral.base_address,
                    0,
                    &[],
                    registers,
                    properties,
                    addresses,
                )?;
            }
            Ok(())
        };
        for instance in &instances {
            validate_instance(instance, &mut addresses)?;
        }
    }
    Ok(addresses)
}

fn validate_register_aliases(input: &str) -> Result<(), Box<dyn Error>> {
    let addresses = expanded_register_map(input)?;
    let mut identities = BTreeSet::new();
    let mut previous_range: Option<(u64, u64, String)> = None;
    for (address, registers) in addresses {
        for register in &registers {
            if !identities.insert(register.identity.clone()) {
                return Err(
                    format!("duplicate expanded register name {}", register.identity).into(),
                );
            }
            let size_bytes = u64::from(register.size_bits).div_ceil(8);
            if address % size_bytes != 0 {
                return Err(format!(
                    "register {} at 0x{address:08x} is not aligned to {size_bytes} bytes",
                    register.identity
                )
                .into());
            }
        }
        let end = address
            .checked_add(
                registers
                    .iter()
                    .map(|register| u64::from(register.size_bits).div_ceil(8))
                    .max()
                    .expect("address map entries are nonempty"),
            )
            .ok_or("SVD register end address overflow")?;
        if let Some((previous_start, previous_end, previous_identity)) = &previous_range
            && address < *previous_end
            && address != *previous_start
        {
            return Err(format!(
                "physical register ranges overlap: {previous_identity} at \
                     0x{previous_start:08x}..0x{previous_end:08x} and {} at \
                     0x{address:08x}..0x{end:08x}",
                registers[0].identity
            )
            .into());
        }
        previous_range = Some((address, end, registers[0].identity.clone()));
        validate_alias_group(address, &registers)?;
    }
    Ok(())
}

fn access_label(access: Option<Access>) -> &'static str {
    access.map(Access::as_str).unwrap_or("unspecified")
}

fn generate_binding_index(input: &str) -> Result<String, Box<dyn Error>> {
    let addresses = expanded_register_map(input)?;
    let mut output = String::from("pac-binding-index 2\ncrate open_esp_radio_esp32s31_pac\n");
    for (address, registers) in addresses {
        for register in registers {
            let peripheral_type = type_binding_name(&register.peripheral);
            let peripheral_module = member_binding_name(&register.peripheral);
            let scope = if register.scope.is_empty() {
                "-".to_owned()
            } else {
                register
                    .scope
                    .iter()
                    .map(|scope| match scope.array_index {
                        Some(index) => format!("{}[{index}]", scope.rust_name),
                        None => scope.rust_name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(".")
            };
            let register_index = register
                .array_index
                .map_or_else(|| "-".to_owned(), |index| index.to_string());
            let alternate = register.alternate_register.as_deref().unwrap_or("-");
            output.push_str(&format!(
                "register 0x{address:08x} {} {} {} {} {} {} {} {} {} {}\n",
                register.size_bits,
                access_label(register.access),
                register.identity,
                register.peripheral,
                peripheral_type,
                peripheral_module,
                scope,
                register.rust_name,
                register_index,
                alternate,
            ));
            for field in register.fields {
                let field_index = field
                    .array_index
                    .map_or_else(|| "-".to_owned(), |index| index.to_string());
                output.push_str(&format!(
                    "field 0x{address:08x} {} {} {} {} {} {} {}\n",
                    register.identity,
                    field.name,
                    field.rust_name,
                    field_index,
                    field.bit_offset,
                    field.bit_width,
                    access_label(field.access),
                ));
            }
        }
    }
    Ok(output)
}

fn validate_structure(input: &str) -> Result<Vec<MmioWindow>, Box<dyn Error>> {
    let document = Document::parse(input)?;
    validate_dimension_order(&document)?;
    validate_register_layout(&document)?;
    validate_write_semantics(&document)?;
    validate_names(&document)?;
    validate_provenance(&document, input)?;
    validate_confidence(input)?;
    let windows = parse_mmio_windows(&document)?;
    validate_evidence_ranges(&document, &windows)?;
    validate_register_aliases(input)?;
    parse_interrupt_snapshots(&document)?;
    parse_full_register_writes(&document)?;
    parse_fixed_register_writes(&document)?;
    parse_fixed_register_images(&document)?;
    parse_register_image_writes(&document)?;
    parse_zero_based_field_writes(&document)?;
    parse_zero_register_writes(&document)?;
    parse_masked_register_modifies(&document)?;
    Ok(windows)
}

fn run() -> Result<(), Box<dyn Error>> {
    let check = match env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [argument] if argument == "--check" => true,
        _ => return Err(USAGE.into()),
    };

    let root = repository_root();
    let output_path = root.join("driver/chips/esp32s31/pac/src/lib.rs");
    let binding_index_path = root.join("svd/esp32s31-radio.bindings");
    let materialized = radio_svd::materialize(&root)?;
    radio_svd::synchronize_aggregate(&materialized, check)?;
    let svd_path = materialized.aggregate_path;
    let input = materialized.contents;
    let generation_input = radio_svd::attach_pac_addon(&input, &materialized.addon_contents)
        .map_err(|error| {
            format!(
                "cannot apply target PAC add-on {}: {error}",
                materialized.addon_path.display()
            )
        })?;
    let windows = validate_structure(&generation_input)?;
    let generation_document = Document::parse(&generation_input)?;
    validate_model_review_sources(&generation_document, &materialized.review_sources)?;
    validate_mmio_windows(&input, &windows)?;
    let platform_svd_path = root.join("svd/esp32s31-platform-radio-deps.svd");
    let platform_input = fs::read_to_string(&platform_svd_path)?;
    svd_parser::parse(&platform_input).map_err(|error| {
        format!(
            "validator-only platform SVD {} is invalid: {error}",
            platform_svd_path.display()
        )
    })?;

    let mut config = Config::default();
    config.edition = RustEdition::E2024;
    config.target = Target::None;
    config.strict = true;
    let interrupt_snapshot_api = generate_interrupt_snapshot_api(&generation_document)?;
    let peripheral_ownership_api = generate_peripheral_ownership_api(&generation_document)?;
    let full_register_write_api = generate_full_register_write_api(&generation_document)?;
    let fixed_register_write_api = generate_fixed_register_write_api(&generation_document)?;
    let fixed_register_image_api = generate_fixed_register_image_api(&generation_document)?;
    let register_image_write_api = generate_register_image_write_api(&generation_document)?;
    let zero_based_field_write_api = generate_zero_based_field_write_api(&generation_document)?;
    let zero_register_write_api = generate_zero_register_write_api(&generation_document)?;
    let masked_register_modify_api = generate_masked_register_modify_api(&generation_document)?;
    let generated = format_generated(&format!(
        "{}{}{}{}{}{}{}{}{}{}{}",
        svd2rust::generate(&input, &config)?.lib_rs,
        interrupt_snapshot_api,
        peripheral_ownership_api,
        full_register_write_api,
        fixed_register_write_api,
        fixed_register_image_api,
        register_image_write_api,
        zero_based_field_write_api,
        zero_register_write_api,
        masked_register_modify_api,
        generate_device_access_api(),
    ))?;
    let binding_index = generate_binding_index(&input)?;

    if check {
        let checked_in = fs::read_to_string(&output_path)?;
        if checked_in != generated {
            return Err(format!(
                "{} differs from {}; run `cargo pac-gen`",
                output_path.display(),
                svd_path.display()
            )
            .into());
        }
        let checked_in_index = fs::read_to_string(&binding_index_path)?;
        if checked_in_index != binding_index {
            return Err(format!(
                "{} differs from {}; run `cargo pac-gen`",
                binding_index_path.display(),
                svd_path.display()
            )
            .into());
        }
    } else {
        fs::create_dir_all(
            output_path
                .parent()
                .expect("generated source path must have a parent"),
        )?;
        fs::write(&output_path, generated)?;
        fs::write(&binding_index_path, binding_index)?;
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("PAC generation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExpandedRegister, MmioWindow, array_binding_name, explicitly_alternate,
        generate_device_access_api, generate_fixed_register_image_api,
        generate_fixed_register_write_api, generate_full_register_write_api,
        generate_interrupt_snapshot_api, generate_masked_register_modify_api,
        generate_peripheral_ownership_api, generate_register_image_write_api,
        generate_zero_based_field_write_api, generate_zero_register_write_api, member_binding_name,
        mmio_window, parse_fixed_register_images, parse_fixed_register_writes,
        parse_full_register_writes, parse_interrupt_snapshots, parse_masked_register_modifies,
        parse_mmio_windows, parse_register_image_writes, parse_zero_based_field_writes,
        parse_zero_register_writes, type_binding_name, validate_alias_group, validate_confidence,
        validate_dimension_order, validate_evidence_ranges, validate_model_review_sources,
        validate_names, validate_provenance, validate_register_aliases, validate_register_layout,
        validate_write_semantics,
    };
    use roxmltree::Document;
    use std::collections::BTreeSet;
    use svd_parser::svd::Access;

    fn windows() -> [MmioWindow; 1] {
        [MmioWindow {
            name: "modem-radio-core".to_owned(),
            start: 0x2010_0000,
            end_exclusive: 0x2020_0000,
        }]
    }

    #[test]
    fn accepts_the_remaining_custom_pac_decode_window() {
        assert_eq!(
            mmio_window(&windows(), 0x2010_0000, 0x2010_0004),
            Some("modem-radio-core")
        );
    }

    #[test]
    fn rejects_holes_and_cross_window_registers() {
        let windows = windows();
        assert_eq!(mmio_window(&windows, 0x2000_0000, 0x2000_0004), None);
        assert_eq!(mmio_window(&windows, 0x2020_0000, 0x2020_0004), None);
        assert_eq!(mmio_window(&windows, 0x201f_fffc, 0x2020_0004), None);
        assert_eq!(mmio_window(&windows, 0x2058_7000, 0x2058_7004), None);
        assert_eq!(mmio_window(&windows, 0x2070_4000, 0x2070_4004), None);
        assert_eq!(mmio_window(&windows, 0x2081_8000, 0x2081_8004), None);
        assert_eq!(mmio_window(&windows, 0x2090_0000, 0x2090_0004), None);
    }

    #[test]
    fn rejects_dimension_metadata_after_the_register_name() {
        let document = Document::parse(
            "<root><register><name>WORDS%s</name><dim>2</dim>\
             <dimIncrement>4</dimIncrement></register></root>",
        )
        .unwrap();
        assert!(validate_dimension_order(&document).is_err());
    }

    #[test]
    fn rejects_overlapping_register_fields() {
        let document = Document::parse(
            "<root><register><name>CONTROL</name><addressOffset>0</addressOffset><size>32</size><fields>\
             <field><name>A</name><bitOffset>0</bitOffset><bitWidth>2</bitWidth></field>\
             <field><name>B</name><bitOffset>1</bitOffset><bitWidth>1</bitWidth></field>\
             </fields></register></root>",
        )
        .unwrap();
        assert!(validate_register_layout(&document).is_err());
    }

    #[test]
    fn rejects_field_outside_register_and_short_array_stride() {
        let document = Document::parse(
            "<root><register><dim>2</dim><dimIncrement>1</dimIncrement>\
             <name>CONTROL%s</name><addressOffset>0</addressOffset><size>16</size><fields>\
             <field><name>A</name><bitOffset>15</bitOffset><bitWidth>2</bitWidth></field>\
             </fields></register></root>",
        )
        .unwrap();
        assert!(validate_register_layout(&document).is_err());
    }

    #[test]
    fn rejects_filler_fields() {
        let document = Document::parse(
            "<root><register><name>CONTROL</name><addressOffset>0</addressOffset><size>32</size><fields>\
             <field><name>PRESERVED_UNKNOWN</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field>\
             </fields></register></root>",
        )
        .unwrap();
        assert!(validate_register_layout(&document).is_err());
    }

    #[test]
    fn write_enum_constraint_requires_a_write_capable_enumeration() {
        let missing = Document::parse(
            "<root><register><name>CONTROL</name><field><name>MODE</name>\
             <bitOffset>0</bitOffset><bitWidth>2</bitWidth>\
             <writeConstraint><useEnumeratedValues>true</useEnumeratedValues></writeConstraint>\
             </field></register></root>",
        )
        .unwrap();
        assert!(validate_write_semantics(&missing).is_err());

        let read_only = Document::parse(
            "<root><register><name>CONTROL</name><field><name>MODE</name>\
             <bitOffset>0</bitOffset><bitWidth>2</bitWidth>\
             <writeConstraint><useEnumeratedValues>true</useEnumeratedValues></writeConstraint>\
             <enumeratedValues><usage>read</usage>\
             <enumeratedValue><name>OFF</name><value>0</value></enumeratedValue>\
             </enumeratedValues></field></register></root>",
        )
        .unwrap();
        assert!(validate_write_semantics(&read_only).is_err());
    }

    #[test]
    fn write_semantics_must_fit_the_field_width() {
        let range = Document::parse(
            "<root><register><name>CONTROL</name><field><name>MODE</name>\
             <bitOffset>0</bitOffset><bitWidth>2</bitWidth>\
             <writeConstraint><range><minimum>0</minimum><maximum>4</maximum></range>\
             </writeConstraint></field></register></root>",
        )
        .unwrap();
        assert!(validate_write_semantics(&range).is_err());

        let enumeration = Document::parse(
            "<root><register><name>CONTROL</name><field><name>MODE</name>\
             <bitOffset>0</bitOffset><bitWidth>2</bitWidth><enumeratedValues>\
             <enumeratedValue><name>TOO_WIDE</name><value>4</value></enumeratedValue>\
             </enumeratedValues></field></register></root>",
        )
        .unwrap();
        assert!(validate_write_semantics(&enumeration).is_err());
    }

    #[test]
    fn interrupt_snapshot_api_is_opaque_and_consumes_the_sample() {
        let document = Document::parse(
            "<device><peripherals><peripheral><name>IRQ_BANK</name><registers>\
             <register><name>STATUS</name><size>32</size><access>read-only</access></register>\
             <register><name>CLEAR</name><size>32</size><access>write-only</access>\
             <modifiedWriteValues>oneToClear</modifiedWriteValues><fields><field>\
             <name>EVENTS</name><bitOffset>0</bitOffset><bitWidth>32</bitWidth>\
             </field></fields></register></registers></peripheral></peripherals>\
             <vendorExtensions><openEspRadioInterruptSnapshots>\
             <snapshot name=\"irq_bank\" peripheral=\"IRQ_BANK\" statusRegister=\"STATUS\" \
             clearRegister=\"CLEAR\" clearField=\"EVENTS\" source=\"TEST\"/>\
             </openEspRadioInterruptSnapshots></vendorExtensions></device>",
        )
        .unwrap();

        assert_eq!(parse_interrupt_snapshots(&document).unwrap().len(), 1);
        let generated = generate_interrupt_snapshot_api(&document).unwrap();
        assert!(generated.contains("pub struct IrqBankSnapshot(u32)"));
        assert!(generated.contains("snapshot: IrqBankSnapshot"));
        assert!(!generated.contains("acknowledge_irq_bank(\n                 registers: &crate::IrqBank,\n                 bits: u32"));
    }

    #[test]
    fn interrupt_snapshots_define_safe_peripheral_ownership_partitions() {
        let document = Document::parse(
            "<device><peripherals>\
             <peripheral><name>RADIO</name></peripheral>\
             <peripheral><name>IRQ_BANK</name><registers>\
             <register><name>STATUS</name><size>32</size><access>read-only</access></register>\
             <register><name>CLEAR</name><size>32</size><access>write-only</access>\
             <modifiedWriteValues>oneToClear</modifiedWriteValues><fields><field>\
             <name>EVENTS</name><bitOffset>0</bitOffset><bitWidth>32</bitWidth>\
             </field></fields></register></registers></peripheral></peripherals>\
             <vendorExtensions><openEspRadioInterruptSnapshots>\
             <snapshot name=\"irq_bank\" peripheral=\"IRQ_BANK\" statusRegister=\"STATUS\" \
             clearRegister=\"CLEAR\" clearField=\"EVENTS\" source=\"TEST\"/>\
             </openEspRadioInterruptSnapshots></vendorExtensions></device>",
        )
        .unwrap();

        let generated = generate_peripheral_ownership_api(&document).unwrap();
        assert!(generated.contains("pub radio: crate::Radio"));
        assert!(generated.contains("pub irq_bank: crate::IrqBank"));
        assert!(generated.contains("let crate::Peripherals"));
        assert!(generated.contains("unsafe { crate::Peripherals::steal() }"));
    }

    #[test]
    fn interrupt_snapshot_rejects_a_non_w1c_clear_register() {
        let document = Document::parse(
            "<device><peripherals><peripheral><name>IRQ_BANK</name><registers>\
             <register><name>STATUS</name><size>32</size><access>read-only</access></register>\
             <register><name>CLEAR</name><size>32</size><access>write-only</access>\
             <fields><field><name>EVENTS</name><bitOffset>0</bitOffset><bitWidth>32</bitWidth>\
             </field></fields></register></registers></peripheral></peripherals>\
             <vendorExtensions><openEspRadioInterruptSnapshots>\
             <snapshot name=\"irq_bank\" peripheral=\"IRQ_BANK\" statusRegister=\"STATUS\" \
             clearRegister=\"CLEAR\" clearField=\"EVENTS\" source=\"TEST\"/>\
             </openEspRadioInterruptSnapshots></vendorExtensions></device>",
        )
        .unwrap();

        assert!(parse_interrupt_snapshots(&document).is_err());
    }

    #[test]
    fn full_register_write_requires_one_constrained_complete_field() {
        let valid = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><name>VALUE</name><size>32</size><access>write-only</access><fields>\
             <field><name>BITS</name><bitOffset>0</bitOffset><bitWidth>32</bitWidth>\
             <writeConstraint><range><minimum>0</minimum><maximum>0xffffffff</maximum>\
             </range></writeConstraint></field></fields></register></registers></peripheral>\
             </peripherals><vendorExtensions><openEspRadioFullRegisterWrites>\
             <write name=\"port_value\" peripheral=\"PORT\" register=\"VALUE\" field=\"BITS\" \
             source=\"TEST\"/></openEspRadioFullRegisterWrites></vendorExtensions></device>",
        )
        .unwrap();
        assert_eq!(parse_full_register_writes(&valid).unwrap().len(), 1);
        assert!(
            generate_full_register_write_api(&valid)
                .unwrap()
                .contains("pub fn port_value(registers: &crate::Port, value: u32)")
        );

        let partial = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><name>VALUE</name><size>32</size><access>write-only</access><fields>\
             <field><name>BITS</name><bitOffset>0</bitOffset><bitWidth>31</bitWidth>\
             <writeConstraint><range><minimum>0</minimum><maximum>0x7fffffff</maximum>\
             </range></writeConstraint></field></fields></register></registers></peripheral>\
             </peripherals><vendorExtensions><openEspRadioFullRegisterWrites>\
             <write name=\"port_value\" peripheral=\"PORT\" register=\"VALUE\" field=\"BITS\" \
             source=\"TEST\"/></openEspRadioFullRegisterWrites></vendorExtensions></device>",
        )
        .unwrap();
        assert!(parse_full_register_writes(&partial).is_err());
    }

    #[test]
    fn fixed_register_write_requires_a_complete_enumerated_field() {
        let valid = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><name>MASK</name><size>32</size><access>read-write</access><fields>\
             <field><name>VALUE</name><bitOffset>0</bitOffset><bitWidth>32</bitWidth>\
             <enumeratedValues><usage>write</usage><enumeratedValue><name>DISABLED</name>\
             <value>0</value></enumeratedValue></enumeratedValues></field></fields></register>\
             </registers></peripheral></peripherals><vendorExtensions>\
             <openEspRadioFixedRegisterWrites><write name=\"disable_port\" peripheral=\"PORT\" \
             register=\"MASK\" field=\"VALUE\" variant=\"DISABLED\" source=\"TEST\"/>\
             </openEspRadioFixedRegisterWrites></vendorExtensions></device>",
        )
        .unwrap();
        assert_eq!(parse_fixed_register_writes(&valid).unwrap().len(), 1);
        let generated = generate_fixed_register_write_api(&valid).unwrap();
        assert!(generated.contains("pub fn disable_port(registers: &crate::Port)"));
        assert!(generated.contains("writer.value().disabled()"));
    }

    #[test]
    fn fixed_register_image_is_exact_and_rejects_modified_write_semantics() {
        let valid = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><dim>2</dim><name>VALUE%s</name><size>32</size>\
             <access>write-only</access></register></registers></peripheral></peripherals>\
             <vendorExtensions><openEspRadioFixedRegisterImages>\
             <write name=\"initialize_port_value\" peripheral=\"PORT\" register=\"VALUE%s\" \
             value=\"0x1234abcd\" source=\"TEST\"/>\
             </openEspRadioFixedRegisterImages></vendorExtensions></device>",
        )
        .unwrap();
        assert_eq!(parse_fixed_register_images(&valid).unwrap().len(), 1);
        let generated = generate_fixed_register_image_api(&valid).unwrap();
        assert!(
            generated
                .contains("pub fn initialize_port_value(registers: &crate::Port, index: usize)")
        );
        assert!(generated.contains("registers.value(index).write_with_zero"));
        assert!(generated.contains("writer.bits(0x1234abcd)"));

        let w1c = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><name>CLEAR</name><size>32</size><access>write-only</access>\
             <modifiedWriteValues>oneToClear</modifiedWriteValues></register>\
             </registers></peripheral></peripherals><vendorExtensions>\
             <openEspRadioFixedRegisterImages><write name=\"clear_port\" peripheral=\"PORT\" \
             register=\"CLEAR\" value=\"1\" source=\"TEST\"/>\
             </openEspRadioFixedRegisterImages></vendorExtensions></device>",
        )
        .unwrap();
        assert!(parse_fixed_register_images(&w1c).is_err());
    }

    #[test]
    fn register_image_write_keeps_array_index_and_rejects_modified_write_semantics() {
        let valid = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><dim>2</dim><name>VECTOR%s</name><size>32</size>\
             <access>read-write</access><fields><field><name>LOW</name><bitOffset>0</bitOffset>\
             <bitWidth>16</bitWidth></field><field><name>HIGH</name><bitOffset>16</bitOffset>\
             <bitWidth>16</bitWidth></field></fields></register></registers></peripheral>\
             </peripherals><vendorExtensions><openEspRadioRegisterImageWrites>\
             <write name=\"publish_port_vector\" peripheral=\"PORT\" register=\"VECTOR%s\" \
             source=\"TEST\"/></openEspRadioRegisterImageWrites></vendorExtensions></device>",
        )
        .unwrap();
        assert_eq!(parse_register_image_writes(&valid).unwrap().len(), 1);
        let generated = generate_register_image_write_api(&valid).unwrap();
        assert!(generated.contains(
            "pub fn publish_port_vector(registers: &crate::Port, index: usize, image: u32)"
        ));
        assert!(generated.contains("registers.vector(index).write_with_zero"));
        assert!(generated.contains("writer.bits(image)"));

        let w1c = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><name>CLEAR</name><size>32</size><access>write-only</access>\
             <modifiedWriteValues>oneToClear</modifiedWriteValues></register>\
             </registers></peripheral></peripherals><vendorExtensions>\
             <openEspRadioRegisterImageWrites><write name=\"publish_clear\" peripheral=\"PORT\" \
             register=\"CLEAR\" source=\"TEST\"/>\
             </openEspRadioRegisterImageWrites></vendorExtensions></device>",
        )
        .unwrap();
        assert!(parse_register_image_writes(&w1c).is_err());
    }

    #[test]
    fn zero_based_field_write_requires_a_complete_range_and_keeps_array_index() {
        let valid = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><dim>4</dim><name>ADDRESS%s</name><size>32</size>\
             <access>read-write</access><fields><field><name>HIGH</name>\
             <bitOffset>0</bitOffset><bitWidth>16</bitWidth><writeConstraint><range>\
             <minimum>0</minimum><maximum>0xffff</maximum></range></writeConstraint>\
             </field><field><name>ENABLE</name><bitOffset>16</bitOffset><bitWidth>1</bitWidth>\
             </field></fields></register></registers></peripheral></peripherals>\
             <vendorExtensions><openEspRadioZeroBasedFieldWrites>\
             <write name=\"port_address_high\" peripheral=\"PORT\" register=\"ADDRESS%s\" \
             field=\"HIGH\" source=\"TEST\"/></openEspRadioZeroBasedFieldWrites>\
             </vendorExtensions></device>",
        )
        .unwrap();
        assert_eq!(parse_zero_based_field_writes(&valid).unwrap().len(), 1);
        let generated = generate_zero_based_field_write_api(&valid).unwrap();
        assert!(generated.contains(
            "pub fn port_address_high(registers: &crate::Port, index: usize, value: u16)"
        ));
        assert!(generated.contains("registers.address(index).write_with_zero"));
        assert!(generated.contains("writer.high().set(value)"));

        let multiple = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><name>COMMAND</name><size>32</size><access>write-only</access><fields>\
             <field><name>BLOCK</name><bitOffset>0</bitOffset><bitWidth>8</bitWidth>\
             <writeConstraint><range><minimum>0</minimum><maximum>0xff</maximum></range>\
             </writeConstraint></field><field><name>DATA</name><bitOffset>16</bitOffset>\
             <bitWidth>8</bitWidth><writeConstraint><range><minimum>0</minimum>\
             <maximum>0xff</maximum></range></writeConstraint></field>\
             <field><name>ENABLE</name><bitOffset>31</bitOffset><bitWidth>1</bitWidth></field>\
             </fields></register>\
             </registers></peripheral></peripherals><vendorExtensions>\
             <openEspRadioZeroBasedFieldWrites><write name=\"port_command\" peripheral=\"PORT\" \
             register=\"COMMAND\" fields=\"BLOCK,DATA,ENABLE\" source=\"TEST\"/>\
             </openEspRadioZeroBasedFieldWrites></vendorExtensions></device>",
        )
        .unwrap();
        let generated = generate_zero_based_field_write_api(&multiple).unwrap();
        assert!(generated.contains(
            "pub fn port_command(registers: &crate::Port, block_value: u8, data_value: u8, enable_value: bool)"
        ));
        assert!(generated.contains(
            "writer.block().set(block_value).data().set(data_value).enable().bit(enable_value)"
        ));

        let partial_range = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><name>VALUE</name><size>32</size><access>write-only</access><fields>\
             <field><name>BITS</name><bitOffset>0</bitOffset><bitWidth>8</bitWidth>\
             <writeConstraint><range><minimum>0</minimum><maximum>7</maximum></range>\
             </writeConstraint></field></fields></register></registers></peripheral>\
             </peripherals><vendorExtensions><openEspRadioZeroBasedFieldWrites>\
             <write name=\"port_value\" peripheral=\"PORT\" register=\"VALUE\" field=\"BITS\" \
             source=\"TEST\"/></openEspRadioZeroBasedFieldWrites></vendorExtensions></device>",
        )
        .unwrap();
        assert!(parse_zero_based_field_writes(&partial_range).is_err());
    }

    #[test]
    fn zero_register_write_is_explicit_and_rejects_modified_write_semantics() {
        let valid = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><name>CONTROL</name><size>32</size><access>read-write</access>\
             </register></registers></peripheral></peripherals><vendorExtensions>\
             <openEspRadioZeroRegisterWrites><write name=\"clear_port_control\" \
             peripheral=\"PORT\" register=\"CONTROL\" source=\"TEST\"/>\
             </openEspRadioZeroRegisterWrites></vendorExtensions></device>",
        )
        .unwrap();
        assert_eq!(parse_zero_register_writes(&valid).unwrap().len(), 1);
        let generated = generate_zero_register_write_api(&valid).unwrap();
        assert!(generated.contains("pub fn clear_port_control(registers: &crate::Port)"));
        assert!(generated.contains("registers.control().write_with_zero(|writer| writer)"));

        let w1c = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><name>CLEAR</name><size>32</size><access>write-only</access>\
             <modifiedWriteValues>oneToClear</modifiedWriteValues></register>\
             </registers></peripheral></peripherals><vendorExtensions>\
             <openEspRadioZeroRegisterWrites><write name=\"clear_port\" peripheral=\"PORT\" \
             register=\"CLEAR\" source=\"TEST\"/></openEspRadioZeroRegisterWrites>\
             </vendorExtensions></device>",
        )
        .unwrap();
        assert!(parse_zero_register_writes(&w1c).is_err());
    }

    #[test]
    fn masked_register_modify_requires_a_complete_disjoint_partition() {
        let valid = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><name>COMMAND</name><size>32</size><access>read-write</access>\
             </register></registers></peripheral></peripherals><vendorExtensions>\
             <openEspRadioMaskedRegisterModifies><modify name=\"publish_command\" \
             peripheral=\"PORT\" register=\"COMMAND\" preserveMask=\"0xfffe0001\" \
             inputMask=\"0x0001fffc\" setMask=\"0x00000002\" source=\"TEST\"/>\
             </openEspRadioMaskedRegisterModifies></vendorExtensions></device>",
        )
        .unwrap();
        assert_eq!(parse_masked_register_modifies(&valid).unwrap().len(), 1);
        let generated = generate_masked_register_modify_api(&valid).unwrap();
        assert!(generated.contains("pub fn publish_command(registers: &crate::Port, input: u32)"));
        assert!(generated.contains("reader.bits() & 0xfffe0001"));
        assert!(generated.contains("input & 0x0001fffc"));
        assert!(generated.contains("| 0x00000002"));

        let overlapping = Document::parse(
            "<device><peripherals><peripheral><name>PORT</name><registers>\
             <register><name>COMMAND</name><size>32</size><access>read-write</access>\
             </register></registers></peripheral></peripherals><vendorExtensions>\
             <openEspRadioMaskedRegisterModifies><modify name=\"publish_command\" \
             peripheral=\"PORT\" register=\"COMMAND\" preserveMask=\"0xfffe0003\" \
             inputMask=\"0x0001fffc\" setMask=\"0x00000002\" source=\"TEST\"/>\
             </openEspRadioMaskedRegisterModifies></vendorExtensions></device>",
        )
        .unwrap();
        assert!(parse_masked_register_modifies(&overlapping).is_err());
    }

    #[test]
    fn device_fence_keeps_unsafe_inside_the_generated_pac() {
        let generated = generate_device_access_api();
        assert!(generated.contains("fence iorw, iorw"));
        assert!(generated.contains("dmb sy"));
        assert!(generated.contains("memw"));
        assert!(generated.contains("compiler_fence"));
    }

    #[test]
    fn rejects_duplicate_names_in_one_scope() {
        let document = Document::parse(
            "<root><peripherals><peripheral><name>P</name><registers>\
             <register><name>A</name></register><register><name>A</name></register>\
             </registers></peripheral></peripherals></root>",
        )
        .unwrap();
        assert!(validate_names(&document).is_err());
    }

    #[test]
    fn rejects_undefined_provenance_references() {
        let input = "<root><description>SOURCE[MISSING]</description>\
                     <source id=\"WINDOWS\">defined</source>\
                     <openEspRadioAddressWindows source=\"WINDOWS\"/></root>";
        let document = Document::parse(input).unwrap();
        assert!(validate_provenance(&document, input).is_err());
    }

    #[test]
    fn rejects_a_model_review_source_missing_from_the_target_addon() {
        let document =
            Document::parse("<root><source id=\"KNOWN\">defined</source></root>").unwrap();
        let sources = BTreeSet::from(["KNOWN".to_owned(), "MISSING".to_owned()]);
        assert!(validate_model_review_sources(&document, &sources).is_err());
    }

    #[test]
    fn rejects_confidence_outside_the_fixed_vocabulary() {
        assert!(validate_confidence("CONFIDENCE[instruction-exatc]").is_err());
        assert!(validate_confidence("CONFIDENCE[instruction-exact]").is_ok());
    }

    #[test]
    fn alternate_register_must_share_physical_scope() {
        let canonical = ExpandedRegister {
            identity: "RADIO.CONTROL".to_owned(),
            peripheral: "RADIO".to_owned(),
            scope: Vec::new(),
            name: "CONTROL".to_owned(),
            rust_name: "control".to_owned(),
            array_index: None,
            size_bits: 32,
            access: Some(Access::ReadWrite),
            alternate_group: None,
            alternate_register: None,
            fields: Vec::new(),
        };
        let mut alternate = canonical.clone();
        alternate.identity = "RADIO.STATUS_VIEW".to_owned();
        alternate.name = "STATUS_VIEW".to_owned();
        alternate.alternate_register = Some("CONTROL".to_owned());
        assert!(explicitly_alternate(&[
            canonical.clone(),
            alternate.clone()
        ]));
        assert!(validate_alias_group(0x2010_0000, &[canonical.clone(), alternate.clone()]).is_ok());
        alternate.access = Some(Access::ReadOnly);
        assert!(
            validate_alias_group(0x2010_0000, &[canonical.clone(), alternate.clone()]).is_err()
        );
        alternate.access = canonical.access;
        alternate.alternate_register = None;
        assert!(
            validate_alias_group(0x2010_0000, &[canonical.clone(), alternate.clone()]).is_err()
        );
        alternate.peripheral = "ANOTHER_SINGLETON".to_owned();
        assert!(!explicitly_alternate(&[canonical, alternate]));
    }

    #[test]
    fn binding_names_match_svd2rust_default_case_and_arrays() {
        assert_eq!(
            member_binding_name("I2C_NUMBER_CONTROL"),
            "i2c_number_control"
        );
        assert_eq!(
            array_binding_name("I2C_NUMBER_WORD%s", None),
            "i2c_number_word"
        );
        assert_eq!(
            array_binding_name("IGNORED%s", Some("RX_BLOCK_ACK_ENTRY")),
            "rx_block_ack_entry"
        );
        assert_eq!(type_binding_name("WIFI_MAC_INTERRUPT"), "WifiMacInterrupt");
    }

    #[test]
    fn rejects_duplicate_names_after_dimension_expansion() {
        let input = "\
            <device schemaVersion=\"1.3\">\
              <name>TEST</name><version>1</version><description>test</description>\
              <addressUnitBits>8</addressUnitBits><width>32</width>\
              <peripherals><peripheral><name>P</name><description>test</description>\
                <baseAddress>0x20100000</baseAddress><registers>\
                  <register><dim>2</dim><dimIncrement>4</dimIncrement><dimIndex>0,0</dimIndex>\
                    <name>R%s</name><description>test</description>\
                    <addressOffset>0</addressOffset><size>32</size><access>read-write</access>\
                  </register>\
                </registers></peripheral></peripherals>\
            </device>";
        assert!(validate_register_aliases(input).is_err());
    }

    #[test]
    fn reads_mmio_windows_from_the_svd_extension() {
        let document = Document::parse(
            "<root><openEspRadioAddressWindows source=\"MAP\">\
             <window name=\"RADIO\" start=\"0x20100000\" endExclusive=\"0x20200000\"/>\
             </openEspRadioAddressWindows></root>",
        )
        .unwrap();
        assert_eq!(
            parse_mmio_windows(&document).unwrap(),
            [MmioWindow {
                name: "RADIO".to_owned(),
                start: 0x2010_0000,
                end_exclusive: 0x2020_0000,
            }]
        );
    }

    #[test]
    fn validates_half_open_evidence_ranges() {
        let document = Document::parse(
            "<root><openEspRadioEvidenceRanges source=\"DEBUG\">\
             <range name=\"FE\" start=\"0x20100000\" endExclusive=\"0x20100058\"/>\
             <range name=\"BB\" start=\"0x20100400\" endExclusive=\"0x2010048c\"/>\
             </openEspRadioEvidenceRanges></root>",
        )
        .unwrap();
        assert!(validate_evidence_ranges(&document, &windows()).is_ok());

        let overlapping = Document::parse(
            "<root><openEspRadioEvidenceRanges source=\"DEBUG\">\
             <range name=\"A\" start=\"0x20100000\" endExclusive=\"0x20100008\"/>\
             <range name=\"B\" start=\"0x20100004\" endExclusive=\"0x2010000c\"/>\
             </openEspRadioEvidenceRanges></root>",
        )
        .unwrap();
        assert!(validate_evidence_ranges(&overlapping, &windows()).is_err());
    }
}
