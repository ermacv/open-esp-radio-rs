//! Cross-record and value invariants for projected function facts.

use std::collections::BTreeSet;

use crate::Result;

use super::{FunctionFact, FunctionInputFact};

pub(super) fn validate(inputs: &[FunctionInputFact], functions: &[FunctionFact]) -> Result<()> {
    let mut input_keys = BTreeSet::new();
    for input in inputs {
        if !input_keys.insert((&input.profile, &input.source)) {
            return Err(crate::Error::invalid(format!(
                "duplicate function fact input {}:{}",
                input.profile, input.source
            )));
        }
    }
    let mut function_keys = BTreeSet::new();
    for function in functions {
        if !input_keys.contains(&(&function.profile, &function.source)) {
            return Err(crate::Error::invalid(format!(
                "function {}:{} refers to an unknown source",
                function.profile, function.identity
            )));
        }
        if !function_keys.insert((&function.profile, &function.identity)) {
            return Err(crate::Error::invalid(format!(
                "duplicate function identity {}:{}",
                function.profile, function.identity
            )));
        }
        if !matches!(
            function.selection.as_str(),
            "symbol-prefix-root" | "reachable-internal"
        ) {
            return Err(crate::Error::invalid(format!(
                "function {}:{} has unsupported selection {:?}",
                function.profile, function.identity, function.selection
            )));
        }
        let mut fields = BTreeSet::new();
        for field in &function.context_fields {
            if field.argument >= super::super::MAX_CONTEXT_ARGUMENTS
                || !matches!(field.width, 8 | 16 | 32 | 64)
            {
                return Err(crate::Error::invalid(format!(
                    "function {}:{} has an invalid context field",
                    function.profile, function.identity
                )));
            }
            if field.reads == 0 && field.writes == 0 {
                return Err(crate::Error::invalid(format!(
                    "function {}:{} has a context field without observed accesses",
                    function.profile, function.identity
                )));
            }
            if !fields.insert((field.argument, field.offset, field.width)) {
                return Err(crate::Error::invalid(format!(
                    "function {}:{} has a duplicate context field",
                    function.profile, function.identity
                )));
            }
        }
        let mut memory_fields = BTreeSet::new();
        for field in &function.memory_fields {
            if !matches!(field.width, 8 | 16 | 32 | 64)
                || field.reads == 0 && field.writes == 0
                || !memory_fields.insert((&field.object, field.offset, field.width))
            {
                return Err(crate::Error::invalid(format!(
                    "function {}:{} has an invalid or duplicate memory-object field",
                    function.profile, function.identity
                )));
            }
        }
    }
    Ok(())
}
