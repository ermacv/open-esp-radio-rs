//! One-shot CMSIS-SVD import into the editable multi-file register model.

use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use svd_rs::{MaybeArray, Peripheral, RegisterCluster, ValidateLevel};

use super::{
    ModelDevice, RegisterFacts, RegisterModelFragment, RegisterModelManifest, ReviewAnnotation,
};
use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegisterModelImportSummary {
    pub(crate) peripherals: usize,
    pub(crate) fragments: usize,
    pub(crate) annotations: usize,
}

fn identifier_from(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        let character = if character.is_ascii_alphanumeric() || character == '_' {
            character
        } else {
            '_'
        };
        if output.is_empty() && character.is_ascii_digit() {
            output.push('_');
        }
        output.push(character);
    }
    if output.is_empty() {
        "UNNAMED".to_owned()
    } else {
        output
    }
}

pub(crate) fn init_register_model(
    facts: &RegisterFacts,
    output_path: &Path,
    address_space: &str,
    project_id: &str,
) -> Result<RegisterModelImportSummary> {
    if output_path.exists() {
        return Err(crate::Error::invalid(format!(
            "refusing to overwrite existing register model {}",
            output_path.display()
        )));
    }
    let output_base = output_path.parent().unwrap_or_else(|| Path::new("."));
    let mut used_files = BTreeSet::new();
    let mut used_peripherals = BTreeSet::new();
    let mut fragment_paths = Vec::with_capacity(facts.ranges.len());
    let mut encoded_fragments = Vec::with_capacity(facts.ranges.len());
    for range in &facts.ranges {
        let peripheral_name = identifier_from(&range.name).to_ascii_uppercase();
        if !used_peripherals.insert(peripheral_name.clone()) {
            return Err(crate::Error::invalid(format!(
                "MMIO ranges produce duplicate peripheral name {peripheral_name:?}; rename the ranges before initializing the model"
            )));
        }
        let peripheral = svd_rs::PeripheralInfo::builder()
            .name(peripheral_name)
            .description(None)
            .base_address(u64::from(range.start))
            .registers(None)
            .build(ValidateLevel::Strict)?;
        let file_name = unique_file_name(&range.name, &mut used_files);
        let relative = PathBuf::from("peripherals").join(&file_name);
        let absolute = output_base.join(&relative);
        if absolute.exists() {
            return Err(crate::Error::invalid(format!(
                "refusing to overwrite existing register fragment {}",
                absolute.display()
            )));
        }
        let fragment = RegisterModelFragment {
            schema: 2,
            peripherals: vec![MaybeArray::Single(peripheral)],
            review: Vec::new(),
        };
        fragment_paths.push(path_text(&relative)?);
        encoded_fragments.push((
            absolute,
            hexadecimal_literals(&toml_edit::ser::to_string_pretty(&fragment)?),
        ));
    }
    let manifest = RegisterModelManifest {
        schema: 2,
        address_space: address_space.to_owned(),
        device: ModelDevice {
            name: identifier_from(project_id).to_ascii_uppercase(),
            version: "0.1".to_owned(),
            description: "Reviewed register model".to_owned(),
            vendor: None,
            vendor_id: None,
            series: None,
            license_text: None,
            cpu: None,
            header_system_filename: None,
            header_definitions_prefix: None,
            address_unit_bits: 8,
            width: 32,
            register_defaults: svd_rs::RegisterProperties::default(),
            svd_schema: "1.3".to_owned(),
            svd_schema_location: "CMSIS-SVD.xsd".to_owned(),
        },
        fragments: fragment_paths,
    };
    write_model_files(output_path, &manifest, &encoded_fragments)?;
    Ok(RegisterModelImportSummary {
        peripherals: encoded_fragments.len(),
        fragments: encoded_fragments.len(),
        annotations: 0,
    })
}

pub(crate) fn import_svd_model(
    input_path: &Path,
    output_path: &Path,
    address_space: &str,
) -> Result<RegisterModelImportSummary> {
    if output_path.exists() {
        return Err(crate::Error::invalid(format!(
            "refusing to overwrite existing register model {}",
            output_path.display()
        )));
    }
    let input = fs::read_to_string(input_path)?;
    let mut device = svd_parser::parse(&input)
        .map_err(|error| format!("cannot import SVD {}: {error}", input_path.display()))
        .map_err(crate::Error::invalid)?;
    let output_base = output_path.parent().unwrap_or_else(|| Path::new("."));
    let mut used_files = BTreeSet::new();
    let mut fragment_paths = Vec::with_capacity(device.peripherals.len());
    let mut encoded_fragments = Vec::with_capacity(device.peripherals.len());
    let mut annotation_count = 0usize;

    for mut peripheral in std::mem::take(&mut device.peripherals) {
        let name = peripheral.name.clone();
        let file_name = unique_file_name(&name, &mut used_files);
        let relative = PathBuf::from("peripherals").join(&file_name);
        let absolute = output_base.join(&relative);
        if absolute.exists() {
            return Err(crate::Error::invalid(format!(
                "refusing to overwrite existing register fragment {}",
                absolute.display()
            )));
        }
        let mut review = Vec::new();
        clean_peripheral(&mut peripheral, &mut review)?;
        annotation_count += review.len();
        let fragment = RegisterModelFragment {
            schema: 2,
            peripherals: vec![peripheral],
            review,
        };
        let encoded = hexadecimal_literals(&toml_edit::ser::to_string_pretty(&fragment)?);
        fragment_paths.push(path_text(&relative)?);
        encoded_fragments.push((absolute, encoded));
    }

    let metadata = ModelDevice {
        name: device.name,
        version: device.version,
        description: device.description,
        vendor: device.vendor,
        vendor_id: device.vendor_id,
        series: device.series,
        license_text: device.license_text,
        cpu: device.cpu,
        header_system_filename: device.header_system_filename,
        header_definitions_prefix: device.header_definitions_prefix,
        address_unit_bits: device.address_unit_bits,
        width: device.width,
        register_defaults: device.default_register_properties,
        svd_schema: device.schema_version,
        svd_schema_location: device.no_namespace_schema_location,
    };
    let manifest = RegisterModelManifest {
        schema: 2,
        address_space: address_space.to_owned(),
        device: metadata,
        fragments: fragment_paths,
    };
    write_model_files(output_path, &manifest, &encoded_fragments)?;

    Ok(RegisterModelImportSummary {
        peripherals: encoded_fragments.len(),
        fragments: encoded_fragments.len(),
        annotations: annotation_count,
    })
}

fn write_model_files(
    output_path: &Path,
    manifest: &RegisterModelManifest,
    fragments: &[(PathBuf, String)],
) -> Result<()> {
    let encoded_manifest = hexadecimal_literals(&toml_edit::ser::to_string_pretty(manifest)?);
    if let Some(parent) = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent.join("peripherals"))?;
    }
    for (path, contents) in fragments {
        write_new(path, contents)?;
    }
    write_new(output_path, &encoded_manifest)
}

fn hexadecimal_literals(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for line in input.lines() {
        let replacement = line.split_once('=').and_then(|(key, value)| {
            matches!(
                key.trim(),
                "baseAddress"
                    | "addressOffset"
                    | "dimIncrement"
                    | "resetValue"
                    | "resetMask"
                    | "minimum"
                    | "maximum"
            )
            .then(|| value.trim().parse::<u64>().ok())
            .flatten()
            .map(|value| format!("{}= 0x{value:X}", &line[..key.len()]))
        });
        output.push_str(replacement.as_deref().unwrap_or(line));
        output.push('\n');
    }
    output
}

fn write_new(path: &Path, contents: &str) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "register model path is not UTF-8: {}",
                path.display()
            ))
        })
}

fn unique_file_name(name: &str, used: &mut BTreeSet<String>) -> String {
    let base = file_stem(name);
    let mut candidate = format!("{base}.toml");
    let mut suffix = 2usize;
    while !used.insert(candidate.clone()) {
        candidate = format!("{base}-{suffix}.toml");
        suffix += 1;
    }
    candidate
}

fn file_stem(name: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('-');
            }
            output.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    if output.is_empty() {
        "peripheral".to_owned()
    } else {
        output
    }
}

fn clean_peripheral(peripheral: &mut Peripheral, review: &mut Vec<ReviewAnnotation>) -> Result<()> {
    let path = peripheral.name.clone();
    clean_description(&path, &mut peripheral.description, review)?;
    for interrupt in &mut peripheral.interrupt {
        clean_description(
            &format!("{path}.interrupt.{}", interrupt.name),
            &mut interrupt.description,
            review,
        )?;
    }
    if let Some(children) = &mut peripheral.registers {
        clean_children(&path, children, review)?;
    }
    Ok(())
}

fn clean_children(
    parent: &str,
    children: &mut [RegisterCluster],
    review: &mut Vec<ReviewAnnotation>,
) -> Result<()> {
    for child in children {
        match child {
            RegisterCluster::Register(register) => {
                let path = format!("{parent}.{}", register.name);
                clean_description(&path, &mut register.description, review)?;
                if let Some(fields) = &mut register.fields {
                    for field in fields {
                        let field_path = format!("{path}.{}", field.name);
                        clean_description(&field_path, &mut field.description, review)?;
                        for values in &mut field.enumerated_values {
                            for value in &mut values.values {
                                clean_description(
                                    &format!("{field_path}.{}", value.name),
                                    &mut value.description,
                                    review,
                                )?;
                            }
                        }
                    }
                }
            }
            RegisterCluster::Cluster(cluster) => {
                let path = format!("{parent}.{}", cluster.name);
                clean_description(&path, &mut cluster.description, review)?;
                clean_children(&path, &mut cluster.children, review)?;
            }
        }
    }
    Ok(())
}

fn clean_description(
    entity: &str,
    description: &mut Option<String>,
    review: &mut Vec<ReviewAnnotation>,
) -> Result<()> {
    let Some(current) = description.take() else {
        return Ok(());
    };
    let sources = annotation_values(&current, "SOURCE")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let provenance = annotation_values(&current, "PROVENANCE")
        .map(parse_provenance)
        .transpose()?;
    let accuracy = annotation_values(&current, "ACCURACY")
        .map(parse_accuracy)
        .transpose()?;
    let completeness = annotation_values(&current, "COMPLETENESS")
        .map(parse_completeness)
        .transpose()?;
    let cleaned = strip_leading_annotations(&current);
    *description = (!cleaned.is_empty()).then_some(cleaned);
    if !sources.is_empty() || provenance.is_some() || accuracy.is_some() || completeness.is_some() {
        review.push(ReviewAnnotation {
            entity: entity.to_owned(),
            sources,
            provenance,
            accuracy,
            completeness,
        });
    }
    Ok(())
}

fn annotation_values<'a>(value: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("{name}[");
    let start = value.find(&prefix)? + prefix.len();
    let end = value[start..].find(']')? + start;
    Some(&value[start..end])
}

fn strip_leading_annotations(value: &str) -> String {
    let mut rest = value.trim();
    loop {
        rest = rest.trim_start_matches([';', '.', ' ']).trim_start();
        let Some((name, tail)) = rest
            .strip_prefix("SOURCE[")
            .map(|tail| ("SOURCE", tail))
            .or_else(|| {
                rest.strip_prefix("PROVENANCE[")
                    .map(|tail| ("PROVENANCE", tail))
                    .or_else(|| {
                        rest.strip_prefix("ACCURACY[")
                            .map(|tail| ("ACCURACY", tail))
                    })
                    .or_else(|| {
                        rest.strip_prefix("COMPLETENESS[")
                            .map(|tail| ("COMPLETENESS", tail))
                    })
            })
        else {
            break;
        };
        let Some(end) = tail.find(']') else {
            break;
        };
        let _ = name;
        rest = &tail[end + 1..];
    }
    rest.trim().to_owned()
}

fn parse_provenance(value: &str) -> Result<crate::FactProvenance> {
    use crate::FactProvenance::*;
    match value {
        "observed" => Ok(Observed),
        "derived" => Ok(Derived),
        "imported" => Ok(Imported),
        "hint" => Ok(Hint),
        "reviewed" => Ok(Reviewed),
        _ => Err(crate::Error::invalid(format!(
            "invalid fact provenance {value:?}"
        ))),
    }
}

fn parse_accuracy(value: &str) -> Result<crate::FactAccuracy> {
    use crate::FactAccuracy::*;
    match value {
        "exact" => Ok(Exact),
        "bounded" => Ok(Bounded),
        "approximate" => Ok(Approximate),
        "unknown" => Ok(Unknown),
        _ => Err(crate::Error::invalid(format!(
            "invalid fact accuracy {value:?}"
        ))),
    }
}

fn parse_completeness(value: &str) -> Result<crate::FactCompleteness> {
    use crate::FactCompleteness::*;
    match value {
        "complete" => Ok(Complete),
        "partial" => Ok(Partial),
        "unknown" => Ok(Unknown),
        _ => Err(crate::Error::invalid(format!(
            "invalid fact completeness {value:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registers::FactRange;

    #[test]
    fn initializes_an_editable_model_without_promoting_discovery_facts() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-register-init-model-{}",
            std::process::id()
        ));
        let output = directory.join("device.toml");
        let facts = RegisterFacts {
            artifacts: Vec::new(),
            ranges: vec![
                FactRange {
                    name: "radio-core".to_owned(),
                    start: 0x1000,
                    end: 0x2000,
                },
                FactRange {
                    name: "radio-aux".to_owned(),
                    start: 0x3000,
                    end: 0x4000,
                },
            ],
            registers: Vec::new(),
        };

        let summary = init_register_model(&facts, &output, "cpu", "fixture").unwrap();
        let model = super::super::RegisterModel::load(&output).unwrap();
        let (_, svd_summary) = model.render_svd().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(summary.peripherals, 2);
        assert_eq!(summary.annotations, 0);
        assert_eq!(svd_summary.peripherals, 2);
        assert_eq!(svd_summary.registers, 0);
    }

    #[test]
    fn rejects_range_names_that_normalize_to_one_peripheral() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-register-init-collision-{}",
            std::process::id()
        ));
        let output = directory.join("device.toml");
        let facts = RegisterFacts {
            artifacts: Vec::new(),
            ranges: vec![
                FactRange {
                    name: "radio-core".to_owned(),
                    start: 0x1000,
                    end: 0x2000,
                },
                FactRange {
                    name: "radio_core".to_owned(),
                    start: 0x3000,
                    end: 0x4000,
                },
            ],
            registers: Vec::new(),
        };

        let error = init_register_model(&facts, &output, "cpu", "fixture").unwrap_err();
        assert!(error.to_string().contains("duplicate peripheral name"));
        assert!(!output.exists());
    }

    #[test]
    fn separates_provenance_from_hardware_description() {
        let mut description = Some(
            "SOURCE[ROM_FN, BLOB_FN]; PROVENANCE[observed]; ACCURACY[exact]; COMPLETENESS[complete]. Enable radio"
                .to_owned(),
        );
        let mut review = Vec::new();
        clean_description("RADIO.CONTROL", &mut description, &mut review).unwrap();
        assert_eq!(description.as_deref(), Some("Enable radio"));
        assert_eq!(review.len(), 1);
        assert_eq!(review[0].sources, ["ROM_FN", "BLOB_FN"]);
        assert_eq!(review[0].provenance, Some(crate::FactProvenance::Observed));
        assert_eq!(review[0].accuracy, Some(crate::FactAccuracy::Exact));
        assert_eq!(
            review[0].completeness,
            Some(crate::FactCompleteness::Complete)
        );
    }

    #[test]
    fn creates_stable_unique_fragment_names() {
        let mut used = BTreeSet::new();
        assert_eq!(unique_file_name("WIFI_MAC", &mut used), "wifi-mac.toml");
        assert_eq!(unique_file_name("WIFI-MAC", &mut used), "wifi-mac-2.toml");
    }

    #[test]
    fn formats_addresses_and_masks_as_hexadecimal() {
        let input = "baseAddress = 537919488\nbitOffset = 3\nmaximum = 4294967295\n";
        assert_eq!(
            hexadecimal_literals(input),
            "baseAddress = 0x20100000\nbitOffset = 3\nmaximum = 0xFFFFFFFF\n"
        );
    }
}
