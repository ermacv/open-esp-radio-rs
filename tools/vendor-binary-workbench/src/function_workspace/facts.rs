//! Minimal stable projection of schema-v32 linked IR used by function review.

use std::{collections::BTreeSet, fs, path::PathBuf};

use serde_json::{Map, Value};

use crate::Result;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionInputFact {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionContextFieldFact {
    pub(crate) argument: u8,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionCallFact {
    pub(crate) kind: String,
    pub(crate) target: String,
    pub(crate) semantic_operation: Option<String>,
    pub(crate) site: Option<u32>,
    pub(crate) arguments: Vec<String>,
    pub(crate) guard_paths: Option<Vec<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionFact {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    pub(crate) selection: String,
    pub(crate) direct_complete: bool,
    pub(crate) call_graph_closed: bool,
    pub(crate) context_projection_complete: bool,
    pub(crate) context_projection_blockers: Vec<String>,
    pub(crate) reachable_functions: Vec<String>,
    pub(crate) calls: Vec<FunctionCallFact>,
    pub(crate) context_fields: Vec<FunctionContextFieldFact>,
    pub(crate) semantic_operations: Vec<String>,
    pub(crate) trampoline_calls: usize,
    pub(crate) event_dispatches: usize,
    pub(crate) pseudo: String,
}

impl FunctionFact {
    pub(crate) fn is_root(&self) -> bool {
        self.selection == "symbol-prefix-root"
    }

    pub(crate) fn review_complete(&self) -> bool {
        self.direct_complete && self.call_graph_closed && self.context_projection_complete
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionFacts {
    pub(crate) inputs: Vec<FunctionInputFact>,
    pub(crate) functions: Vec<FunctionFact>,
}

impl FunctionFacts {
    pub(crate) fn load(reports: &[(String, PathBuf)]) -> Result<Self> {
        let mut inputs = Vec::new();
        let mut functions = Vec::new();
        for (profile, path) in reports {
            let input = fs::read_to_string(path)?;
            let root: Value = serde_json::from_str(&input)?;
            let root = object(&root, "linked-IR root")?;
            let context = format!("linked-IR profile {profile:?}");
            if integer(root, "schema_version", &context)? != 32
                || string(root, "command", &context)? != "ir export"
            {
                return Err(format!(
                    "function workspace requires a schema-v32 ir export report for profile {profile:?}"
                )
                .into());
            }
            if boolean(root, "completeness_claim", &context)? {
                return Err(format!(
                    "linked-IR profile {profile:?} makes an unsupported completeness claim"
                )
                .into());
            }
            inputs.extend(parse_inputs(profile, root, &context)?);
            functions.extend(parse_functions(profile, root, &context)?);
        }
        inputs.sort();
        functions.sort_by(|left, right| {
            (&left.profile, &left.identity).cmp(&(&right.profile, &right.identity))
        });
        validate(&inputs, &functions)?;
        Ok(Self { inputs, functions })
    }

    pub(crate) fn function(
        &self,
        profile: &str,
        source: &str,
        identity: &str,
    ) -> Option<&FunctionFact> {
        self.functions.iter().find(|function| {
            function.profile == profile
                && function.source == source
                && function.identity == identity
        })
    }

    pub(crate) fn root_functions(&self) -> impl Iterator<Item = &FunctionFact> {
        self.functions.iter().filter(|function| function.is_root())
    }
}

fn parse_inputs(
    profile: &str,
    root: &Map<String, Value>,
    context: &str,
) -> Result<Vec<FunctionInputFact>> {
    array(root, "artifacts", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.artifacts[{index}]");
            let artifact = object(value, &context)?;
            let evidence = object(
                artifact
                    .get("artifact")
                    .ok_or_else(|| format!("{context} requires artifact evidence"))?,
                &format!("{context}.artifact"),
            )?;
            Ok(FunctionInputFact {
                profile: profile.to_owned(),
                source: string(artifact, "source", &context)?.to_owned(),
                sha256: sha256(evidence, "sha256", &context)?.to_owned(),
            })
        })
        .collect()
}

fn parse_functions(
    profile: &str,
    root: &Map<String, Value>,
    context: &str,
) -> Result<Vec<FunctionFact>> {
    array(root, "functions", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.functions[{index}]");
            let function = object(value, &context)?;
            let summary = object(
                function
                    .get("effect_summary")
                    .ok_or_else(|| format!("{context} requires effect_summary"))?,
                &format!("{context}.effect_summary"),
            )?;
            Ok(FunctionFact {
                profile: profile.to_owned(),
                source: string(function, "source", &context)?.to_owned(),
                identity: string(function, "identity", &context)?.to_owned(),
                member: optional_string(function, "member", &context)?,
                symbol: string(function, "symbol", &context)?.to_owned(),
                selection: string(function, "selection", &context)?.to_owned(),
                direct_complete: boolean(function, "complete", &context)?,
                call_graph_closed: boolean(summary, "call_graph_closed", &context)?,
                context_projection_complete: boolean(
                    summary,
                    "context_projection_complete",
                    &context,
                )?,
                context_projection_blockers: strings(
                    summary,
                    "context_projection_blockers",
                    &context,
                )?,
                reachable_functions: strings(summary, "reachable_functions", &context)?,
                calls: parse_calls(function, &context)?,
                context_fields: parse_fields(summary, &context)?,
                semantic_operations: array(summary, "semantic_operations", &context)?
                    .iter()
                    .enumerate()
                    .map(|(operation_index, value)| {
                        let operation_context =
                            format!("{context}.semantic_operations[{operation_index}]");
                        Ok(string(
                            object(value, &operation_context)?,
                            "operation",
                            &operation_context,
                        )?
                        .to_owned())
                    })
                    .collect::<Result<Vec<_>>>()?,
                trampoline_calls: array(summary, "trampoline_calls", &context)?.len(),
                event_dispatches: array(summary, "event_dispatches", &context)?.len(),
                pseudo: string(function, "pseudo", &context)?.to_owned(),
            })
        })
        .collect()
}

fn parse_calls(function: &Map<String, Value>, context: &str) -> Result<Vec<FunctionCallFact>> {
    array(function, "calls", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.calls[{index}]");
            let call = object(value, &context)?;
            Ok(FunctionCallFact {
                kind: string(call, "kind", &context)?.to_owned(),
                target: string(call, "target", &context)?.to_owned(),
                semantic_operation: optional_string(call, "semantic_operation", &context)?,
                site: optional_hex_u32(call, "site", &context)?,
                arguments: strings(call, "arguments", &context)?,
                guard_paths: optional_guard_paths(call, &context)?,
            })
        })
        .collect()
}

fn optional_guard_paths(call: &Map<String, Value>, context: &str) -> Result<Option<Vec<String>>> {
    let Some(paths) = call.get("cfg_guard_paths") else {
        return Err(format!("{context} requires cfg_guard_paths").into());
    };
    if paths.is_null() {
        return Ok(None);
    }
    let paths = paths
        .as_array()
        .ok_or_else(|| format!("{context}.cfg_guard_paths must be an array or null"))?;
    paths
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.cfg_guard_paths[{index}]");
            let path = object(value, &context)?;
            let guards = array(path, "guards", &context)?;
            let literals = guards
                .iter()
                .enumerate()
                .map(|(guard_index, guard)| {
                    let guard_context = format!("{context}.guards[{guard_index}]");
                    let guard = object(guard, &guard_context)?;
                    let condition = string(guard, "condition", &guard_context)?;
                    Ok(if boolean(guard, "taken", &guard_context)? {
                        format!("({condition})")
                    } else {
                        format!("!({condition})")
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(if literals.is_empty() {
                "true".to_owned()
            } else {
                literals.join(" && ")
            })
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn parse_fields(
    summary: &Map<String, Value>,
    context: &str,
) -> Result<Vec<FunctionContextFieldFact>> {
    array(summary, "context_fields", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.context_fields[{index}]");
            let field = object(value, &context)?;
            Ok(FunctionContextFieldFact {
                argument: integer(field, "argument", &context)?
                    .try_into()
                    .map_err(|_| format!("invalid argument in {context}"))?,
                offset: signed(field, "offset", &context)?
                    .try_into()
                    .map_err(|_| format!("invalid offset in {context}"))?,
                width: integer(field, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("invalid width in {context}"))?,
                reads: count(field, "reads", &context)?,
                writes: count(field, "writes", &context)?,
                write_mask: hex_u32(field, "write_mask", &context)?,
            })
        })
        .collect()
}

fn validate(inputs: &[FunctionInputFact], functions: &[FunctionFact]) -> Result<()> {
    let mut input_keys = BTreeSet::new();
    for input in inputs {
        if !input_keys.insert((&input.profile, &input.source)) {
            return Err(format!(
                "duplicate function fact input {}:{}",
                input.profile, input.source
            )
            .into());
        }
    }
    let mut function_keys = BTreeSet::new();
    for function in functions {
        if !input_keys.contains(&(&function.profile, &function.source)) {
            return Err(format!(
                "function {}:{} refers to an unknown source",
                function.profile, function.identity
            )
            .into());
        }
        if !function_keys.insert((&function.profile, &function.identity)) {
            return Err(format!(
                "duplicate function identity {}:{}",
                function.profile, function.identity
            )
            .into());
        }
        if !matches!(
            function.selection.as_str(),
            "symbol-prefix-root" | "reachable-internal"
        ) {
            return Err(format!(
                "function {}:{} has unsupported selection {:?}",
                function.profile, function.identity, function.selection
            )
            .into());
        }
        let mut fields = BTreeSet::new();
        for field in &function.context_fields {
            if field.argument >= 8 || !matches!(field.width, 8 | 16 | 32 | 64) {
                return Err(format!(
                    "function {}:{} has an invalid context field",
                    function.profile, function.identity
                )
                .into());
            }
            if field.reads == 0 && field.writes == 0 {
                return Err(format!(
                    "function {}:{} has a context field without observed accesses",
                    function.profile, function.identity
                )
                .into());
            }
            if !fields.insert((field.argument, field.offset, field.width)) {
                return Err(format!(
                    "function {}:{} has a duplicate context field",
                    function.profile, function.identity
                )
                .into());
            }
        }
    }
    Ok(())
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object").into())
}

fn array<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Result<&'a Vec<Value>> {
    object
        .get(key)
        .and_then(Value::as_array)
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
    object
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("{context}.{key} must be a non-empty string or null").into())
        })
        .transpose()
}

fn strings(object: &Map<String, Value>, key: &str, context: &str) -> Result<Vec<String>> {
    array(object, key, context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{context}.{key}[{index}] must be a string").into())
        })
        .collect()
}

fn boolean(object: &Map<String, Value>, key: &str, context: &str) -> Result<bool> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{context} requires boolean {key:?}").into())
}

fn integer(object: &Map<String, Value>, key: &str, context: &str) -> Result<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{context} requires non-negative integer {key:?}").into())
}

fn signed(object: &Map<String, Value>, key: &str, context: &str) -> Result<i64> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{context} requires integer {key:?}").into())
}

fn count(object: &Map<String, Value>, key: &str, context: &str) -> Result<usize> {
    integer(object, key, context)?
        .try_into()
        .map_err(|_| format!("invalid count {key:?} in {context}").into())
}

fn sha256<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Result<&'a str> {
    let value = string(object, key, context)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(format!("{context} has invalid lowercase SHA-256").into());
    }
    Ok(value)
}

fn hex_u32(object: &Map<String, Value>, key: &str, context: &str) -> Result<u32> {
    match object.get(key) {
        Some(Value::Number(value)) => value
            .as_u64()
            .and_then(|value| value.try_into().ok())
            .ok_or_else(|| format!("{context}.{key} must be a u32").into()),
        Some(Value::String(value)) => value
            .strip_prefix("0x")
            .and_then(|value| u32::from_str_radix(value, 16).ok())
            .ok_or_else(|| format!("{context}.{key} must be a hexadecimal u32 string").into()),
        _ => Err(format!("{context}.{key} must be a u32").into()),
    }
}

fn optional_hex_u32(object: &Map<String, Value>, key: &str, context: &str) -> Result<Option<u32>> {
    if object.get(key).is_some_and(Value::is_null) {
        return Ok(None);
    }
    hex_u32(object, key, context).map(Some)
}
