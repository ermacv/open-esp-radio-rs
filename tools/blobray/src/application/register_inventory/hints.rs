//! Exact call-argument/string references, with explicitly conditional printf
//! interpretation. Symbol spelling alone never establishes hardware semantics.
use super::*;

pub(super) struct StringObject {
    pub artifact: String,
    pub address: u64,
    pub bytes: Vec<u8>,
    pub evidence: String,
}

pub(super) fn call_hints(call: &Value, artifact: &str, objects: &[StringObject]) -> Vec<Value> {
    let mut hints = Vec::new();
    for (position, argument) in call["arguments"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(address) = argument
            .as_str()
            .and_then(|value| value.strip_prefix("const:"))
            .and_then(|value| number(&json!(value)))
        else {
            continue;
        };
        for object in objects.iter().filter(|object| object.artifact == artifact) {
            let Some(offset) = address
                .checked_sub(object.address)
                .and_then(|offset| usize::try_from(offset).ok())
            else {
                continue;
            };
            let Some(bytes) = object.bytes.get(offset..) else {
                continue;
            };
            let Some(end) = bytes.iter().position(|byte| *byte == 0) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes[..end]) else {
                continue;
            };
            if text.is_empty()
                || !text
                    .chars()
                    .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
            {
                continue;
            }
            hints.push(json!({"relation":"exact-call-argument-to-string","string":text,"string_evidence":object.evidence,"string_offset":offset,"format_argument":position,"site":call["site"],"target":call["target"],"argument_bit_sources":call["argument_bit_sources"],"printf_interpretation":{"confidence":"conditional-on-printf-compatible-callee-and-ABI","specifiers":printf_specifiers(text, position + 1)}}));
        }
    }
    hints
}

/// Only single-word conversions are assigned argument indices. Unsupported
/// positional/length/variadic layouts return unknown, retaining the whole string.
fn printf_specifiers(text: &str, first_argument: usize) -> Option<Vec<Value>> {
    let bytes = text.as_bytes();
    let mut output = Vec::new();
    let mut cursor = 0;
    let mut argument = first_argument;
    while cursor < bytes.len() {
        if bytes[cursor] != b'%' {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        if bytes.get(cursor) == Some(&b'%') {
            cursor += 1;
            continue;
        }
        while bytes
            .get(cursor)
            .is_some_and(|byte| b"-+ #0".contains(byte) || byte.is_ascii_digit() || *byte == b'.')
        {
            cursor += 1;
        }
        let conversion = *bytes.get(cursor)?;
        if !b"diuoxXcsp".contains(&conversion) {
            return None;
        }
        cursor += 1;
        output.push(
            json!({"argument":argument,"specifier":&text[start..cursor],"string_offset":start}),
        );
        argument += 1;
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unguarded_debug_argument_links_string_and_bit_projection() {
        let objects = vec![StringObject {
            artifact: "a".to_owned(),
            address: 4096,
            bytes: b"status=%08x %% done\0".to_vec(),
            evidence: "string-source".to_owned(),
        }];
        let call = json!({"site":32,"target":"printf","arguments":["const:0x1000","mmio-value"],"argument_bit_sources":[{"position":1,"address":8192,"register_bit":4,"output_bit":0,"token":7}]});
        let hints = call_hints(&call, "a", &objects);
        assert_eq!(hints.len(), 1);
        assert_eq!(
            hints[0]["printf_interpretation"]["specifiers"][0]["argument"],
            1
        );
        assert_eq!(hints[0]["argument_bit_sources"][0]["token"], 7);
        assert!(call_hints(&call, "different-revision", &objects).is_empty());
        assert!(printf_specifiers("wide=%llx", 1).is_none());
        assert!(printf_specifiers("indexed=%2$x", 1).is_none());
    }
}
