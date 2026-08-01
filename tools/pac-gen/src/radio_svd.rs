//! Deterministic assembly of the ESP32-S31 radio SVD from physical-block fragments.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    path::{Component, Path, PathBuf},
};

use roxmltree::Document;

const SOURCE_DIRECTORY: &str = "svd/esp32s31-radio";
const MANIFEST_FILE: &str = "manifest.xml";
const TEMPLATE_FILE: &str = "device.svd.in";
const AGGREGATE_FILE: &str = "svd/esp32s31-radio.svd";

#[derive(Clone, Debug, Eq, PartialEq)]
struct FragmentSpec {
    id: String,
    path: PathBuf,
    start: u64,
    end_exclusive: u64,
}

#[derive(Debug)]
pub(crate) struct AssembledRadioSvd {
    pub(crate) aggregate_path: PathBuf,
    pub(crate) contents: String,
}

fn parse_number(value: &str) -> Result<u64, Box<dyn Error>> {
    if let Some(value) = value.strip_prefix("0x") {
        Ok(u64::from_str_radix(value, 16)?)
    } else {
        Ok(value.parse()?)
    }
}

fn local_path(value: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = Path::new(value);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("fragment path {value:?} is not a safe relative path").into());
    }
    Ok(path.to_owned())
}

fn required_attribute<'a>(
    node: roxmltree::Node<'a, 'a>,
    name: &str,
) -> Result<&'a str, Box<dyn Error>> {
    node.attribute(name)
        .ok_or_else(|| format!("{} is missing attribute {name:?}", node.tag_name().name()).into())
}

fn parse_manifest(input: &str) -> Result<Vec<FragmentSpec>, Box<dyn Error>> {
    let document = Document::parse(input)?;
    let root = document.root_element();
    if !root.has_tag_name("openEspRadioSvdManifest") {
        return Err("radio SVD manifest has the wrong root element".into());
    }

    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut fragments = Vec::new();
    let mut previous_end = None;
    for node in root.children().filter(roxmltree::Node::is_element) {
        if !node.has_tag_name("fragment") {
            return Err(format!(
                "unsupported element {:?} in radio SVD manifest",
                node.tag_name().name()
            )
            .into());
        }
        let id = required_attribute(node, "id")?.to_owned();
        let path = local_path(required_attribute(node, "path")?)?;
        let start = parse_number(required_attribute(node, "start")?)?;
        let end_exclusive = parse_number(required_attribute(node, "endExclusive")?)?;
        if start >= end_exclusive {
            return Err(format!("fragment {id} has an empty or reversed address window").into());
        }
        if previous_end.is_some_and(|end| start < end) {
            return Err(
                format!("fragment {id} starts before the preceding physical window ends").into(),
            );
        }
        if !ids.insert(id.clone()) {
            return Err(format!("duplicate radio SVD fragment id {id:?}").into());
        }
        if !paths.insert(path.clone()) {
            return Err(format!("duplicate radio SVD fragment path {}", path.display()).into());
        }
        previous_end = Some(end_exclusive);
        fragments.push(FragmentSpec {
            id,
            path,
            start,
            end_exclusive,
        });
    }
    if fragments.is_empty() {
        return Err("radio SVD manifest contains no fragments".into());
    }
    Ok(fragments)
}

fn direct_child_text<'a>(
    node: roxmltree::Node<'a, 'a>,
    name: &str,
) -> Result<&'a str, Box<dyn Error>> {
    node.children()
        .find(|child| child.has_tag_name(name))
        .and_then(|child| child.text())
        .ok_or_else(|| format!("peripheral is missing {name}").into())
}

fn parse_fragment(
    spec: &FragmentSpec,
    input: &str,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let document = Document::parse(input)?;
    let root = document.root_element();
    if !root.has_tag_name("openEspRadioPeripheralFragment") {
        return Err(format!("fragment {} has the wrong root element", spec.id).into());
    }
    if required_attribute(root, "id")? != spec.id {
        return Err(format!("fragment {} does not match its manifest id", spec.id).into());
    }

    let mut peripherals = BTreeMap::new();
    for node in root.children().filter(roxmltree::Node::is_element) {
        if !node.has_tag_name("peripheral") {
            return Err(format!(
                "fragment {} contains unsupported element {:?}",
                spec.id,
                node.tag_name().name()
            )
            .into());
        }
        let name = direct_child_text(node, "name")?.trim().to_owned();
        let base_address = parse_number(direct_child_text(node, "baseAddress")?.trim())?;
        if !(spec.start..spec.end_exclusive).contains(&base_address) {
            return Err(format!(
                "peripheral {name} base address 0x{base_address:08x} is outside fragment {} window 0x{:08x}..0x{:08x}",
                spec.id, spec.start, spec.end_exclusive
            )
            .into());
        }
        let raw = input[node.range()].to_owned();
        if peripherals.insert(name.clone(), raw).is_some() {
            return Err(format!("duplicate peripheral {name:?} in fragment {}", spec.id).into());
        }
    }
    if peripherals.is_empty() {
        return Err(format!("radio SVD fragment {} contains no peripherals", spec.id).into());
    }
    Ok(peripherals)
}

fn assemble_template(
    template: &str,
    peripherals: &BTreeMap<String, String>,
) -> Result<String, Box<dyn Error>> {
    let document = Document::parse(template)?;
    let root = document.root_element();
    if !root.has_tag_name("device") {
        return Err("radio SVD device template has the wrong root element".into());
    }

    let mut replacements = Vec::new();
    let mut referenced = BTreeSet::new();
    for node in document
        .descendants()
        .filter(|node| node.has_tag_name("openEspRadioPeripheral"))
    {
        if !node
            .parent()
            .is_some_and(|parent| parent.has_tag_name("peripherals"))
        {
            return Err("radio SVD peripheral reference is outside <peripherals>".into());
        }
        let name = required_attribute(node, "name")?;
        let raw = peripherals
            .get(name)
            .ok_or_else(|| format!("template refers to unknown peripheral {name:?}"))?;
        if !referenced.insert(name.to_owned()) {
            return Err(format!("template refers to peripheral {name:?} more than once").into());
        }
        replacements.push((node.range(), raw));
    }
    if replacements.is_empty() {
        return Err("radio SVD device template contains no peripheral references".into());
    }
    if let Some(name) = peripherals.keys().find(|name| !referenced.contains(*name)) {
        return Err(format!("fragment peripheral {name:?} is not present in the template").into());
    }

    let mut output = String::with_capacity(
        template.len()
            + replacements
                .iter()
                .map(|(range, raw)| raw.len().saturating_sub(range.len()))
                .sum::<usize>(),
    );
    let mut cursor = 0;
    for (range, raw) in replacements {
        output.push_str(&template[cursor..range.start]);
        output.push_str(raw);
        cursor = range.end;
    }
    output.push_str(&template[cursor..]);
    Ok(output)
}

pub(crate) fn assemble(repository_root: &Path) -> Result<AssembledRadioSvd, Box<dyn Error>> {
    let source_directory = repository_root.join(SOURCE_DIRECTORY);
    let manifest = fs::read_to_string(source_directory.join(MANIFEST_FILE))?;
    let specs = parse_manifest(&manifest)?;
    let mut peripherals = BTreeMap::new();
    for spec in specs {
        let input = fs::read_to_string(source_directory.join(&spec.path))?;
        for (name, peripheral) in parse_fragment(&spec, &input)? {
            if peripherals.insert(name.clone(), peripheral).is_some() {
                return Err(format!("peripheral {name:?} occurs in more than one fragment").into());
            }
        }
    }
    let template = fs::read_to_string(source_directory.join(TEMPLATE_FILE))?;
    Ok(AssembledRadioSvd {
        aggregate_path: repository_root.join(AGGREGATE_FILE),
        contents: assemble_template(&template, &peripherals)?,
    })
}

pub(crate) fn synchronize_aggregate(
    assembled: &AssembledRadioSvd,
    check: bool,
) -> Result<(), Box<dyn Error>> {
    let checked_in = fs::read_to_string(&assembled.aggregate_path)?;
    if checked_in == assembled.contents {
        return Ok(());
    }
    if check {
        return Err(format!(
            "{} differs from the physical-block SVD fragments; run `cargo pac-gen`",
            assembled.aggregate_path.display()
        )
        .into());
    }
    fs::write(&assembled.aggregate_path, &assembled.contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{FragmentSpec, assemble_template, local_path, parse_fragment, parse_manifest};
    use std::path::PathBuf;

    #[test]
    fn manifest_requires_ordered_non_overlapping_physical_windows() {
        let valid = "<openEspRadioSvdManifest>\
            <fragment id=\"FE\" path=\"peripherals/fe.xml\" start=\"0x20100000\" endExclusive=\"0x20104000\"/>\
            <fragment id=\"MAC\" path=\"peripherals/wifi-mac.xml\" start=\"0x20104000\" endExclusive=\"0x20107000\"/>\
            </openEspRadioSvdManifest>";
        assert_eq!(parse_manifest(valid).unwrap().len(), 2);
        assert!(
            parse_manifest(&valid.replace(
                "path=\"peripherals/wifi-mac.xml\" start=\"0x20104000\"",
                "path=\"peripherals/wifi-mac.xml\" start=\"0x20103000\""
            ))
            .is_err()
        );
        assert!(local_path("../outside.xml").is_err());
    }

    #[test]
    fn fragment_rejects_a_peripheral_from_another_physical_window() {
        let spec = FragmentSpec {
            id: "FE".to_owned(),
            path: PathBuf::from("fe.xml"),
            start: 0x2010_0000,
            end_exclusive: 0x2010_4000,
        };
        let fragment = "<openEspRadioPeripheralFragment id=\"FE\">\
            <peripheral><name>WRONG</name><baseAddress>0x20104000</baseAddress></peripheral>\
            </openEspRadioPeripheralFragment>";
        assert!(parse_fragment(&spec, fragment).is_err());
    }

    #[test]
    fn template_requires_an_exact_bijection_with_fragment_peripherals() {
        let template = "<device><peripherals><openEspRadioPeripheral name=\"FE\"/>\
            </peripherals></device>";
        let peripherals = [(
            "FE".to_owned(),
            "<peripheral><name>FE</name><baseAddress>0x20100000</baseAddress></peripheral>"
                .to_owned(),
        )]
        .into_iter()
        .collect();
        let assembled = assemble_template(template, &peripherals).unwrap();
        assert!(assembled.contains("<peripheral><name>FE</name>"));
        assert!(!assembled.contains("openEspRadioPeripheral"));

        let extra = [
            ("FE".to_owned(), peripherals["FE"].clone()),
            ("UNUSED".to_owned(), peripherals["FE"].clone()),
        ]
        .into_iter()
        .collect();
        assert!(assemble_template(template, &extra).is_err());
    }
}
