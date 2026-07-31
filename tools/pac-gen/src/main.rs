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

const USAGE: &str = "usage: cargo pac-gen [--check]";
const ALLOWED_CONFIDENCE_VALUES: &[&str] = &[
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
    scope: String,
    name: String,
    size_bits: u32,
    access: Option<Access>,
    alternate_group: Option<String>,
    alternate_register: Option<String>,
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

fn expanded_register_addresses(
    peripheral_name: &str,
    peripheral_base: u64,
    parent_offset: u64,
    scope: &str,
    children: &[RegisterCluster],
    inherited: RegisterProperties,
    addresses: &mut BTreeMap<u64, Vec<ExpandedRegister>>,
) -> Result<(), Box<dyn Error>> {
    for child in children {
        match child {
            RegisterCluster::Register(register) => {
                let instances = match register {
                    MaybeArray::Single(info) => vec![info.clone()],
                    MaybeArray::Array(info, dim) => {
                        svd_parser::svd::register::expand(info, dim).collect()
                    }
                };
                for info in instances {
                    let properties = inherited_properties(inherited, info.properties);
                    let size_bits = properties
                        .size
                        .ok_or_else(|| format!("register {} has no inherited size", info.name))?;
                    if let Some(fields) = &info.fields {
                        let mut names = BTreeSet::new();
                        let mut occupied = 0_u128;
                        for field in fields {
                            let instances = match field {
                                MaybeArray::Single(info) => vec![info.clone()],
                                MaybeArray::Array(info, dim) => {
                                    svd_parser::svd::field::expand(info, dim).collect()
                                }
                            };
                            for field in instances {
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
                            }
                        }
                    }
                    let address = peripheral_base
                        .checked_add(parent_offset)
                        .and_then(|base| base.checked_add(u64::from(info.address_offset)))
                        .ok_or("SVD register address overflow")?;
                    let identity = if scope.is_empty() {
                        format!("{peripheral_name}.{}", info.name)
                    } else {
                        format!("{peripheral_name}.{scope}.{}", info.name)
                    };
                    addresses
                        .entry(address)
                        .or_default()
                        .push(ExpandedRegister {
                            identity,
                            peripheral: peripheral_name.to_owned(),
                            scope: scope.to_owned(),
                            name: info.name.clone(),
                            size_bits,
                            access: properties.access,
                            alternate_group: info.alternate_group.clone(),
                            alternate_register: info.alternate_register.clone(),
                        });
                }
            }
            RegisterCluster::Cluster(cluster) => {
                let instances = match cluster {
                    MaybeArray::Single(info) => vec![info.clone()],
                    MaybeArray::Array(info, dim) => {
                        svd_parser::svd::cluster::expand(info, dim).collect()
                    }
                };
                for info in instances {
                    let properties =
                        inherited_properties(inherited, info.default_register_properties);
                    let child_scope = if scope.is_empty() {
                        info.name.clone()
                    } else {
                        format!("{scope}.{}", info.name)
                    };
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

fn validate_register_aliases(input: &str) -> Result<(), Box<dyn Error>> {
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
                    "",
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
        if let Some((previous_start, previous_end, previous_identity)) = &previous_range {
            if address < *previous_end && address != *previous_start {
                return Err(format!(
                    "physical register ranges overlap: {previous_identity} at \
                     0x{previous_start:08x}..0x{previous_end:08x} and {} at \
                     0x{address:08x}..0x{end:08x}",
                    registers[0].identity
                )
                .into());
            }
        }
        previous_range = Some((address, end, registers[0].identity.clone()));
        validate_alias_group(address, &registers)?;
    }
    Ok(())
}

fn validate_structure(input: &str) -> Result<Vec<MmioWindow>, Box<dyn Error>> {
    let document = Document::parse(input)?;
    validate_dimension_order(&document)?;
    validate_register_layout(&document)?;
    validate_names(&document)?;
    validate_provenance(&document, input)?;
    validate_confidence(input)?;
    let windows = parse_mmio_windows(&document)?;
    validate_evidence_ranges(&document, &windows)?;
    validate_register_aliases(input)?;
    Ok(windows)
}

fn run() -> Result<(), Box<dyn Error>> {
    let check = match env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [argument] if argument == "--check" => true,
        _ => return Err(USAGE.into()),
    };

    let root = repository_root();
    let svd_path = root.join("svd/esp32s31-radio.svd");
    let output_path = root.join("crates/esp32s31/svd/src/lib.rs");
    let input = fs::read_to_string(&svd_path)?;
    let windows = validate_structure(&input)?;
    validate_mmio_windows(&input, &windows)?;

    let mut config = Config::default();
    config.edition = RustEdition::E2024;
    config.target = Target::None;
    config.strict = true;
    let generated = format_generated(&svd2rust::generate(&input, &config)?.lib_rs)?;

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
    } else {
        fs::create_dir_all(
            output_path
                .parent()
                .expect("generated source path must have a parent"),
        )?;
        fs::write(&output_path, generated)?;
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
        ExpandedRegister, MmioWindow, explicitly_alternate, mmio_window, parse_mmio_windows,
        validate_alias_group, validate_confidence, validate_dimension_order,
        validate_evidence_ranges, validate_names, validate_provenance, validate_register_aliases,
        validate_register_layout,
    };
    use roxmltree::Document;
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
    fn rejects_confidence_outside_the_fixed_vocabulary() {
        assert!(validate_confidence("CONFIDENCE[instruction-exatc]").is_err());
        assert!(validate_confidence("CONFIDENCE[instruction-exact]").is_ok());
    }

    #[test]
    fn alternate_register_must_share_physical_scope() {
        let canonical = ExpandedRegister {
            identity: "RADIO.CONTROL".to_owned(),
            peripheral: "RADIO".to_owned(),
            scope: String::new(),
            name: "CONTROL".to_owned(),
            size_bits: 32,
            access: Some(Access::ReadWrite),
            alternate_group: None,
            alternate_register: None,
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
