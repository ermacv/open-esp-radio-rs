//! Small, format-independent parsers shared by manifests, SVD and the CLI.

pub(crate) fn u32_literal(value: &str) -> Option<u32> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}
