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
use svd2rust::{
    config::{Config, RustEdition},
    Target,
};
use svd_parser::svd::{MaybeArray, RegisterCluster, RegisterProperties};

const USAGE: &str = "usage: cargo pac-gen [--check]";
const INTENTIONAL_REGISTER_ALIAS_ADDRESSES: &[u64] = &[
    0x2010_0894,
    0x2010_4004,
    0x2010_4048,
    0x2010_4110,
    0x2010_4400,
    0x2010_4c1c,
    0x2010_4c98,
    0x2010_4d34,
    0x2010_4d38,
    0x2010_4d44,
    0x2010_4d48,
    0x2010_4d54,
    0x2010_4d58,
    0x2010_4d64,
    0x2010_4d68,
    0x2010_539c,
    0x2010_53b0,
    0x2010_53c0,
    0x2010_5418,
    0x2010_542c,
    0x2010_543c,
    0x2010_5494,
    0x2010_54a8,
    0x2010_54b8,
    0x2010_5510,
    0x2010_5524,
    0x2010_5534,
    0x2010_7128,
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct MmioWindow {
    name: String,
    start: u64,
    end_exclusive: u64,
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
        .args(["--edition", "2021"])
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

fn validate_field_overlaps(document: &Document<'_>) -> Result<(), Box<dyn Error>> {
    for register in document
        .descendants()
        .filter(|node| node.has_tag_name("register"))
    {
        let Some(fields) = register.children().find(|node| node.has_tag_name("fields")) else {
            continue;
        };
        let mut occupied = 0_u128;
        for field in fields.children().filter(|node| node.has_tag_name("field")) {
            let offset = parse_u64(child_text(field, "bitOffset")?, "field bitOffset")?;
            let width = parse_u64(child_text(field, "bitWidth")?, "field bitWidth")?;
            if width == 0 || offset.checked_add(width).is_none_or(|end| end > 128) {
                return Err(format!(
                    "register {} field {} has invalid bit range {offset}+{width}",
                    child_text(register, "name")?,
                    child_text(field, "name")?
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
                    "register {} field {} overlaps another field",
                    child_text(register, "name")?,
                    child_text(field, "name")?
                )
                .into());
            }
            occupied |= mask;
        }
    }
    Ok(())
}

fn source_references(text: &str) -> BTreeSet<&str> {
    let mut references = BTreeSet::new();
    let mut remaining = text;
    while let Some(start) = remaining.find("SOURCE[") {
        remaining = &remaining[start + "SOURCE[".len()..];
        let Some(end) = remaining.find(']') else {
            break;
        };
        for source in remaining[..end].split(',').map(str::trim) {
            if !source.is_empty() {
                references.insert(source);
            }
        }
        remaining = &remaining[end + 1..];
    }
    references
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
    for reference in source_references(input) {
        if !definitions.contains(reference) {
            return Err(format!("SOURCE references undefined provenance id {reference}").into());
        }
    }
    let window_source = document
        .descendants()
        .find(|node| node.has_tag_name("openEspRadioAddressWindows"))
        .and_then(|node| node.attribute("source"))
        .ok_or("openEspRadioAddressWindows has no source")?;
    if !definitions.contains(window_source) {
        return Err(
            format!("MMIO windows reference undefined provenance id {window_source}").into(),
        );
    }
    Ok(())
}

fn expanded_register_addresses(
    peripheral_name: &str,
    peripheral_base: u64,
    parent_offset: u64,
    children: &[RegisterCluster],
    addresses: &mut BTreeMap<u64, Vec<String>>,
) -> Result<(), Box<dyn Error>> {
    for child in children {
        match child {
            RegisterCluster::Register(register) => {
                let (info, dim) = match register {
                    MaybeArray::Single(info) => (info, None),
                    MaybeArray::Array(info, dim) => (info, Some(dim)),
                };
                let count = dim.map_or(1, |dim| dim.dim);
                let increment = dim.map_or(0, |dim| dim.dim_increment);
                for index in 0..count {
                    let offset = info
                        .address_offset
                        .checked_add(index.saturating_mul(increment))
                        .ok_or("SVD register-array offset overflow")?;
                    let address = peripheral_base
                        .checked_add(parent_offset)
                        .and_then(|base| base.checked_add(u64::from(offset)))
                        .ok_or("SVD register address overflow")?;
                    addresses
                        .entry(address)
                        .or_default()
                        .push(format!("{peripheral_name}.{}[{index}]", info.name));
                }
            }
            RegisterCluster::Cluster(cluster) => {
                let (info, dim) = match cluster {
                    MaybeArray::Single(info) => (info, None),
                    MaybeArray::Array(info, dim) => (info, Some(dim)),
                };
                let count = dim.map_or(1, |dim| dim.dim);
                let increment = dim.map_or(0, |dim| dim.dim_increment);
                for index in 0..count {
                    let offset = info
                        .address_offset
                        .checked_add(index.saturating_mul(increment))
                        .ok_or("SVD cluster-array offset overflow")?;
                    expanded_register_addresses(
                        peripheral_name,
                        peripheral_base,
                        parent_offset + u64::from(offset),
                        &info.children,
                        addresses,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn validate_register_aliases(input: &str) -> Result<(), Box<dyn Error>> {
    let device = svd_parser::parse(input)?;
    let mut addresses = BTreeMap::new();
    for peripheral in &device.peripherals {
        let validate_instance = |base_address: u64,
                                 addresses: &mut BTreeMap<u64, Vec<String>>|
         -> Result<(), Box<dyn Error>> {
            if let Some(registers) = &peripheral.registers {
                expanded_register_addresses(
                    &peripheral.name,
                    base_address,
                    0,
                    registers,
                    addresses,
                )?;
            }
            Ok(())
        };
        match peripheral {
            MaybeArray::Single(info) => validate_instance(info.base_address, &mut addresses)?,
            MaybeArray::Array(info, dim) => {
                for index in 0..dim.dim {
                    validate_instance(
                        info.base_address + u64::from(index.saturating_mul(dim.dim_increment)),
                        &mut addresses,
                    )?;
                }
            }
        }
    }
    let actual = addresses
        .iter()
        .filter_map(|(address, names)| (names.len() > 1).then_some(*address))
        .collect::<BTreeSet<_>>();
    let allowed = INTENTIONAL_REGISTER_ALIAS_ADDRESSES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if actual != allowed {
        let unexpected = actual.difference(&allowed).copied().collect::<Vec<_>>();
        let stale = allowed.difference(&actual).copied().collect::<Vec<_>>();
        return Err(format!(
            "register alias set changed; unexpected={unexpected:#x?}, stale={stale:#x?}"
        )
        .into());
    }
    for address in &allowed {
        let names = &addresses[address];
        if names.len() != 2 {
            return Err(format!(
                "intentional register alias 0x{address:08x} has {} identities, expected exactly 2: {names:?}",
                names.len()
            )
            .into());
        }
    }
    Ok(())
}

fn validate_structure(input: &str) -> Result<Vec<MmioWindow>, Box<dyn Error>> {
    let document = Document::parse(input)?;
    validate_dimension_order(&document)?;
    validate_field_overlaps(&document)?;
    validate_provenance(&document, input)?;
    let windows = parse_mmio_windows(&document)?;
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
    let output_path = root.join("crates/open-esp-radio-svd-esp32s31/src/lib.rs");
    let input = fs::read_to_string(&svd_path)?;
    let windows = validate_structure(&input)?;
    validate_mmio_windows(&input, &windows)?;

    let mut config = Config::default();
    config.edition = RustEdition::E2021;
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
        mmio_window, parse_mmio_windows, validate_dimension_order, validate_field_overlaps,
        validate_provenance, MmioWindow,
    };
    use roxmltree::Document;

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
            "<root><register><name>CONTROL</name><fields>\
             <field><name>A</name><bitOffset>0</bitOffset><bitWidth>2</bitWidth></field>\
             <field><name>B</name><bitOffset>1</bitOffset><bitWidth>1</bitWidth></field>\
             </fields></register></root>",
        )
        .unwrap();
        assert!(validate_field_overlaps(&document).is_err());
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
}
