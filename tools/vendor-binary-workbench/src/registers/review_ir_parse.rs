//! Strict projection of schema-v32 linked-IR JSON into register-review evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::{Map, Value};

use super::review_ir::{
    ReviewFieldEvidence, ReviewIrRegister, ReviewPredicateEvidence, ReviewSemanticEvidence,
};
use crate::{Result, error::WorkbenchError, parse_u32};

#[tracing::instrument(name = "load_register_review_ir", fields(path = %path.display()))]
pub(super) fn parse_report(path: &Path) -> Result<Vec<ReviewIrRegister>> {
    let input = fs::read_to_string(path)?;
    parse_report_text(path, &input).map_err(|error| {
        WorkbenchError::manifest_document("linked-IR review report", path, &input, error)
    })
}

fn parse_report_text(path: &Path, input: &str) -> Result<Vec<ReviewIrRegister>> {
    let root: Value = serde_json::from_str(input)?;
    let root = object(&root, "linked-IR root")?;
    if integer(root, "schema_version", "linked-IR report")? != 32 {
        return Err(format!(
            "register review requires linked-IR schema 32 in {}",
            path.display()
        )
        .into());
    }
    if string(root, "command", "linked-IR report")? != "ir export" {
        return Err(format!("{} is not an ir export report", path.display()).into());
    }
    if boolean(root, "completeness_claim", "linked-IR report")?
        || boolean(root, "mmio_field_semantics_claim", "linked-IR report")?
    {
        return Err(format!(
            "linked-IR review input {} makes an unsupported completeness or field-semantics claim",
            path.display()
        )
        .into());
    }
    let mut seen = BTreeSet::new();
    array(root, "mmio_registers", "linked-IR report")?
        .iter()
        .enumerate()
        .map(|(index, register)| {
            let context = format!("mmio_registers[{index}]");
            let register = parse_register(register, &context)?;
            let key = (register.address, register.width);
            if !seen.insert(key) {
                return Err(format!(
                    "duplicate linked-IR register at {:#010x}/{} in {}",
                    key.0,
                    key.1,
                    path.display()
                )
                .into());
            }
            Ok(register)
        })
        .collect()
}

fn parse_register(value: &Value, context: &str) -> Result<ReviewIrRegister> {
    let value = object(value, context)?;
    let address = address(value, "address", context)?;
    let width = integer(value, "width", context)?
        .try_into()
        .map_err(|_| format!("invalid register width in {context}"))?;
    if !matches!(width, 8 | 16 | 32) {
        return Err(format!("unsupported register width {width} in {context}").into());
    }
    let mut fields = BTreeMap::new();
    for (index, field) in array(value, "field_candidates", context)?
        .iter()
        .enumerate()
    {
        let field_context = format!("{context}.field_candidates[{index}]");
        let field = parse_field(field, width, &field_context)?;
        let key = (
            field.least_significant_bit,
            field.most_significant_bit,
            field.mask,
        );
        if fields.insert(key, field).is_some() {
            return Err(format!("duplicate field candidate in {field_context}").into());
        }
    }
    Ok(ReviewIrRegister {
        address,
        width,
        names: string_set(value, "names", context)?,
        functions: string_set(value, "functions", context)?,
        fields,
    })
}

fn parse_field(value: &Value, width: u8, context: &str) -> Result<ReviewFieldEvidence> {
    let value = object(value, context)?;
    let lsb = integer(value, "least_significant_bit", context)?
        .try_into()
        .map_err(|_| format!("invalid least-significant bit in {context}"))?;
    let msb = integer(value, "most_significant_bit", context)?
        .try_into()
        .map_err(|_| format!("invalid most-significant bit in {context}"))?;
    let mask = address(value, "mask", context)?;
    if lsb > msb || msb >= width || contiguous_mask(lsb, msb) != mask {
        return Err(format!("invalid field bit range or mask in {context}").into());
    }
    Ok(ReviewFieldEvidence {
        least_significant_bit: lsb,
        most_significant_bit: msb,
        mask,
        write_shapes: count(value, "write_shapes", context)?,
        predicate_shapes: count(value, "predicate_shapes", context)?,
        poll_shapes: count(value, "poll_shapes", context)?,
        functions: string_set(value, "functions", context)?,
        access_functions: string_set(value, "access_functions", context)?,
        predicate_functions: string_set(value, "predicate_functions", context)?,
        predicate_evidence: array(value, "predicate_evidence", context)?
            .iter()
            .enumerate()
            .map(|(index, evidence)| {
                parse_predicate_evidence(
                    evidence,
                    &format!("{context}.predicate_evidence[{index}]"),
                )
            })
            .collect::<Result<_>>()?,
        semantic_operations: string_set(value, "semantic_operations", context)?,
        semantic_roots: string_set(value, "semantic_roots", context)?,
        semantic_evidence: array(value, "semantic_evidence", context)?
            .iter()
            .enumerate()
            .map(|(index, evidence)| {
                parse_semantic_evidence(evidence, &format!("{context}.semantic_evidence[{index}]"))
            })
            .collect::<Result<_>>()?,
    })
}

fn parse_predicate_evidence(value: &Value, context: &str) -> Result<ReviewPredicateEvidence> {
    let value = object(value, context)?;
    Ok(ReviewPredicateEvidence {
        kind: string(value, "kind", context)?.to_owned(),
        function: string(value, "function", context)?.to_owned(),
        producer_path: string_list(value, "producer_path", context)?,
        condition: string(value, "condition", context)?.to_owned(),
        effective_operation: optional_string(value, "effective_operation", context)?,
        register_comparison_value: optional_address(value, "register_comparison_value", context)?,
    })
}

fn parse_semantic_evidence(value: &Value, context: &str) -> Result<ReviewSemanticEvidence> {
    let value = object(value, context)?;
    Ok(ReviewSemanticEvidence {
        kind: string(value, "kind", context)?.to_owned(),
        root: string(value, "root", context)?.to_owned(),
        operation: string(value, "operation", context)?.to_owned(),
        action_target: string(value, "action_target", context)?.to_owned(),
        action_origin: string(value, "action_origin", context)?.to_owned(),
        predicate_function: string(value, "predicate_function", context)?.to_owned(),
        path_expression: string(value, "path_expression", context)?.to_owned(),
        residual_path_expression: string(value, "residual_path_expression", context)?.to_owned(),
        condition: string(value, "condition", context)?.to_owned(),
        effective_operation: string(value, "effective_operation", context)?.to_owned(),
    })
}

fn contiguous_mask(lsb: u8, msb: u8) -> u32 {
    let width = msb - lsb + 1;
    if width == 32 {
        u32::MAX
    } else {
        ((1_u32 << width) - 1) << lsb
    }
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object").into())
}

fn array<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Result<&'a [Value]> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{context} requires array {key:?}").into())
}

fn string<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{context} requires non-empty string {key:?}").into())
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Option<String>> {
    match object.get(key) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value.clone())),
        _ => Err(format!("{context} requires string or null {key:?}").into()),
    }
}

fn integer(object: &Map<String, Value>, key: &str, context: &str) -> Result<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{context} requires non-negative integer {key:?}").into())
}

fn count(object: &Map<String, Value>, key: &str, context: &str) -> Result<usize> {
    integer(object, key, context)?
        .try_into()
        .map_err(|_| format!("invalid count {key:?} in {context}").into())
}

fn boolean(object: &Map<String, Value>, key: &str, context: &str) -> Result<bool> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{context} requires boolean {key:?}").into())
}

fn address(object: &Map<String, Value>, key: &str, context: &str) -> Result<u32> {
    match object.get(key) {
        Some(Value::Number(value)) => value
            .as_u64()
            .and_then(|value| value.try_into().ok())
            .ok_or_else(|| format!("invalid numeric address {value} in {context}").into()),
        Some(Value::String(value)) => {
            parse_u32(value).ok_or_else(|| format!("invalid address {value:?} in {context}").into())
        }
        _ => Err(format!("{context} requires u32 address {key:?}").into()),
    }
}

fn optional_address(object: &Map<String, Value>, key: &str, context: &str) -> Result<Option<u32>> {
    match object.get(key) {
        Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .and_then(|value| value.try_into().ok())
            .map(Some)
            .ok_or_else(|| format!("invalid numeric address {value} in {context}").into()),
        Some(Value::String(value)) => parse_u32(value)
            .map(Some)
            .ok_or_else(|| format!("invalid address {value:?} in {context}").into()),
        _ => Err(format!("{context} requires u32 address or null {key:?}").into()),
    }
}

fn string_list(object: &Map<String, Value>, key: &str, context: &str) -> Result<Vec<String>> {
    array(object, key, context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!("{context}.{key}[{index}] must be a non-empty string").into()
                })
        })
        .collect()
}

fn string_set(object: &Map<String, Value>, key: &str, context: &str) -> Result<BTreeSet<String>> {
    Ok(string_list(object, key, context)?.into_iter().collect())
}
