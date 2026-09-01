use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};
use rustc_demangle::try_demangle;
use serde::{Deserialize, Serialize};

use crate::{AuditReport, Error, Result};

const STACK_SIZES_SECTION: &str = ".stack_sizes";
const DEFAULT_REPORTED_FRAME_COUNT: usize = 20;

/// Target-owned policy applied to compiler-emitted stack-frame metadata.
///
/// This intentionally limits an individual function frame. LLVM's
/// `.stack_sizes` metadata does not prove a complete worst-case call chain,
/// especially through indirect calls, but it reliably rejects large locals
/// and oversized generated async poll frames before firmware is flashed.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackBudget {
    pub schema: u32,
    pub stack_start_symbol: String,
    pub stack_end_symbol: String,
    pub warn_frame_bytes: u64,
    pub max_frame_bytes: u64,
    pub max_move_bytes: u64,
    pub runtime_cpu0_minimum_free_bytes: u32,
    pub runtime_cpu1_minimum_free_bytes: u32,
    #[serde(default = "default_reported_frame_count")]
    pub reported_frame_count: usize,
    #[serde(default)]
    pub reviewed_frames: Vec<ReviewedStackFrame>,
}

/// Explicit exception for one understood generated frame above the ordinary
/// review threshold. The selector is matched against complete demangled
/// function names; crate disambiguator hashes therefore do not enter policy.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedStackFrame {
    pub function_contains: String,
    #[serde(default)]
    pub source_ends_with: Vec<String>,
    pub max_bytes: u64,
    pub reason: String,
}

impl StackBudget {
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        let budget: Self = toml_edit::de::from_str(&source).map_err(|source| Error::Policy {
            path: path.to_owned(),
            source,
        })?;
        budget.validate()?;
        Ok(budget)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != 3 {
            return Err(Error::InvalidPolicy(format!(
                "unsupported stack policy schema {}; expected 3",
                self.schema
            )));
        }
        if self.stack_start_symbol.is_empty() || self.stack_end_symbol.is_empty() {
            return Err(Error::InvalidPolicy(
                "stack bound symbols must not be empty".into(),
            ));
        }
        if self.warn_frame_bytes == 0 || self.max_frame_bytes == 0 || self.max_move_bytes == 0 {
            return Err(Error::InvalidPolicy(
                "stack warning, frame and move budgets must be greater than zero".into(),
            ));
        }
        if self.warn_frame_bytes > self.max_frame_bytes {
            return Err(Error::InvalidPolicy(
                "stack warning threshold must not exceed the hard frame budget".into(),
            ));
        }
        if self.runtime_cpu0_minimum_free_bytes == 0 || self.runtime_cpu1_minimum_free_bytes == 0 {
            return Err(Error::InvalidPolicy(
                "runtime minimum free stack budgets must be greater than zero".into(),
            ));
        }
        if self.reported_frame_count == 0 {
            return Err(Error::InvalidPolicy(
                "reported stack frame count must be greater than zero".into(),
            ));
        }
        for reviewed in &self.reviewed_frames {
            if reviewed.function_contains.is_empty() || reviewed.reason.is_empty() {
                return Err(Error::InvalidPolicy(
                    "reviewed stack frames require a selector and reason".into(),
                ));
            }
            if reviewed.source_ends_with.iter().any(String::is_empty) {
                return Err(Error::InvalidPolicy(
                    "reviewed stack frame source suffix must not be empty".into(),
                ));
            }
            if reviewed.max_bytes <= self.warn_frame_bytes
                || reviewed.max_bytes > self.max_frame_bytes
            {
                return Err(Error::InvalidPolicy(format!(
                    "reviewed frame `{}` must have a limit above the warning threshold and at or below the hard frame budget",
                    reviewed.function_contains
                )));
            }
        }
        Ok(())
    }
}

const fn default_reported_frame_count() -> usize {
    DEFAULT_REPORTED_FRAME_COUNT
}

#[derive(Clone, Debug, Serialize)]
pub struct StackReport {
    pub schema: u32,
    pub elf: PathBuf,
    pub stack_start: u64,
    pub stack_end: u64,
    pub stack_capacity: u64,
    pub warn_frame_bytes: u64,
    pub max_frame_bytes: u64,
    pub measured_frame_count: usize,
    pub largest_frames: Vec<StackFrame>,
    pub violations: Vec<StackFrame>,
    pub audit: AuditReport,
}

#[derive(Clone, Debug, Serialize)]
pub struct StackFrame {
    pub address: u64,
    pub size: u64,
    pub functions: Vec<String>,
    pub source: Option<StackSourceLocation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StackSourceLocation {
    pub file: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

pub fn analyze_stack(elf_path: &Path, budget: &StackBudget) -> Result<StackReport> {
    budget.validate()?;
    let bytes = fs::read(elf_path).map_err(|source| Error::Read {
        path: elf_path.to_owned(),
        source,
    })?;
    let elf = object::File::parse(bytes.as_slice()).map_err(|error| Error::Elf {
        path: elf_path.to_owned(),
        message: error.to_string(),
    })?;

    let symbols = symbol_addresses(&elf);
    let stack_start = required_symbol(&symbols, &budget.stack_start_symbol, elf_path)?;
    let stack_end = required_symbol(&symbols, &budget.stack_end_symbol, elf_path)?;
    if stack_start <= stack_end {
        return Err(Error::InvalidPolicy(format!(
            "stack symbols describe an empty or upward-growing range: {}={stack_start:#x}, {}={stack_end:#x}",
            budget.stack_start_symbol, budget.stack_end_symbol
        )));
    }

    let function_names = function_names_by_address(&elf);
    let source_loader = addr2line::Loader::new(elf_path).ok();
    let address_bytes = if elf.is_64() { 8 } else { 4 };
    let mut frames_by_address = BTreeMap::<u64, StackFrame>::new();
    let mut section_count = 0_usize;
    for section in elf.sections() {
        if section.name().ok() != Some(STACK_SIZES_SECTION) {
            continue;
        }
        section_count += 1;
        let data = section.data().map_err(|error| Error::Elf {
            path: elf_path.to_owned(),
            message: format!("cannot read {STACK_SIZES_SECTION}: {error}"),
        })?;
        for (address, size) in decode_stack_sizes(data, address_bytes, elf.is_little_endian())
            .map_err(|message| Error::Elf {
                path: elf_path.to_owned(),
                message,
            })?
        {
            let functions = function_names
                .get(&address)
                .cloned()
                .unwrap_or_else(|| vec![format!("<unknown function at {address:#x}>")]);
            frames_by_address
                .entry(address)
                .and_modify(|frame| {
                    frame.size = frame.size.max(size);
                    for function in &functions {
                        if !frame.functions.contains(function) {
                            frame.functions.push(function.clone());
                        }
                    }
                })
                .or_insert(StackFrame {
                    address,
                    size,
                    functions,
                    source: source_loader
                        .as_ref()
                        .and_then(|loader| loader.find_location(address).ok().flatten())
                        .and_then(|location| {
                            Some(StackSourceLocation {
                                file: location.file?.to_owned(),
                                line: location.line,
                                column: location.column,
                            })
                        }),
                });
        }
    }
    if section_count == 0 {
        return Err(Error::Elf {
            path: elf_path.to_owned(),
            message: format!(
                "missing {STACK_SIZES_SECTION}; compile firmware with `-Z emit-stack-sizes`"
            ),
        });
    }

    let measured_frame_count = frames_by_address.len();
    let mut frames = frames_by_address.into_values().collect::<Vec<_>>();
    frames.sort_by_key(|frame| (core::cmp::Reverse(frame.size), frame.address));
    let mut violations = Vec::new();
    let mut audit = AuditReport::default();
    for frame in frames
        .iter()
        .filter(|frame| frame.size > budget.warn_frame_bytes)
    {
        let reviewed = budget
            .reviewed_frames
            .iter()
            .find(|reviewed| reviewed_frame_matches(frame, reviewed));
        let error = if frame.size > budget.max_frame_bytes {
            Some(format!(
                "frame {} is {} bytes, exceeding the {}-byte hard budget",
                compact_function(frame),
                frame.size,
                budget.max_frame_bytes
            ))
        } else if let Some(reviewed) = reviewed {
            if frame.size > reviewed.max_bytes {
                Some(format!(
                    "reviewed frame {} grew to {} bytes, exceeding its {}-byte limit (`{}`)",
                    compact_function(frame),
                    frame.size,
                    reviewed.max_bytes,
                    reviewed.function_contains
                ))
            } else {
                audit.warnings.push(format!(
                    "reviewed frame {} is {} bytes within its {}-byte limit: {}",
                    compact_function(frame),
                    frame.size,
                    reviewed.max_bytes,
                    reviewed.reason
                ));
                None
            }
        } else {
            Some(format!(
                "unreviewed frame {} is {} bytes, above the {}-byte review threshold",
                compact_function(frame),
                frame.size,
                budget.warn_frame_bytes
            ))
        };
        if let Some(error) = error {
            violations.push(frame.clone());
            audit.errors.push(error);
        }
    }
    let largest_frames = frames
        .into_iter()
        .take(budget.reported_frame_count)
        .collect::<Vec<_>>();

    Ok(StackReport {
        schema: 1,
        elf: elf_path.to_owned(),
        stack_start,
        stack_end,
        stack_capacity: stack_start - stack_end,
        warn_frame_bytes: budget.warn_frame_bytes,
        max_frame_bytes: budget.max_frame_bytes,
        measured_frame_count,
        largest_frames,
        violations,
        audit,
    })
}

fn reviewed_frame_matches(frame: &StackFrame, reviewed: &ReviewedStackFrame) -> bool {
    frame
        .functions
        .iter()
        .any(|function| function.contains(&reviewed.function_contains))
        && (reviewed.source_ends_with.is_empty()
            || frame.source.as_ref().is_some_and(|source| {
                reviewed
                    .source_ends_with
                    .iter()
                    .any(|suffix| source.file.ends_with(suffix))
            }))
}

pub fn audit_stack(report: &StackReport) -> Result<()> {
    if report.audit.errors.is_empty() {
        return Ok(());
    }
    Err(Error::StackAudit(report.audit.errors.join("\n")))
}

pub fn render_stack_report(report: &StackReport) -> String {
    let mut output = format!(
        "Stack-frame report: {}\n\nstack={:#010x}..{:#010x} capacity={}\nwarn-frame={} max-frame={} measured-frames={}\n\nLargest frames\n",
        report.elf.display(),
        report.stack_end,
        report.stack_start,
        bytes(report.stack_capacity),
        bytes(report.warn_frame_bytes),
        bytes(report.max_frame_bytes),
        report.measured_frame_count,
    );
    for frame in &report.largest_frames {
        output.push_str(&format!(
            "  {:>10}  {:#010x}  {}{}\n",
            bytes(frame.size),
            frame.address,
            compact_function(frame),
            compact_source(frame),
        ));
    }
    output.push('\n');
    output.push_str(&crate::render_audit(&report.audit));
    output
}

fn symbol_addresses(elf: &object::File<'_>) -> HashMap<String, u64> {
    elf.symbols()
        .chain(elf.dynamic_symbols())
        .filter_map(|symbol| {
            symbol
                .name()
                .ok()
                .map(|name| (name.to_owned(), symbol.address()))
        })
        .collect()
}

fn required_symbol(symbols: &HashMap<String, u64>, name: &str, elf_path: &Path) -> Result<u64> {
    symbols.get(name).copied().ok_or_else(|| Error::Elf {
        path: elf_path.to_owned(),
        message: format!("missing stack-bound symbol `{name}`"),
    })
}

fn function_names_by_address(elf: &object::File<'_>) -> HashMap<u64, Vec<String>> {
    let mut names = HashMap::<u64, Vec<String>>::new();
    for symbol in elf.symbols() {
        if !symbol.is_definition() || symbol.kind() != SymbolKind::Text || symbol.address() == 0 {
            continue;
        }
        let Ok(raw) = symbol.name() else {
            continue;
        };
        let demangled = try_demangle(raw)
            .map(|name| name.to_string())
            .unwrap_or_else(|_| raw.to_owned());
        let at_address = names.entry(symbol.address()).or_default();
        if !at_address.contains(&demangled) {
            at_address.push(demangled);
        }
    }
    names
}

fn decode_stack_sizes(
    data: &[u8],
    address_bytes: usize,
    little_endian: bool,
) -> std::result::Result<Vec<(u64, u64)>, String> {
    if !matches!(address_bytes, 4 | 8) {
        return Err(format!(
            "unsupported ELF address width: {address_bytes} bytes"
        ));
    }
    let mut entries = Vec::new();
    let mut offset = 0_usize;
    while offset < data.len() {
        let address_end = offset.saturating_add(address_bytes);
        let Some(address_data) = data.get(offset..address_end) else {
            return Err(format!(
                "truncated {STACK_SIZES_SECTION} address at byte {offset}"
            ));
        };
        let mut padded = [0_u8; 8];
        if little_endian {
            padded[..address_bytes].copy_from_slice(address_data);
            offset = address_end;
            let address = u64::from_le_bytes(padded);
            let (size, consumed) = decode_uleb128(&data[offset..], offset)?;
            offset += consumed;
            entries.push((address, size));
        } else {
            padded[8 - address_bytes..].copy_from_slice(address_data);
            offset = address_end;
            let address = u64::from_be_bytes(padded);
            let (size, consumed) = decode_uleb128(&data[offset..], offset)?;
            offset += consumed;
            entries.push((address, size));
        }
    }
    Ok(entries)
}

fn decode_uleb128(data: &[u8], section_offset: usize) -> std::result::Result<(u64, usize), String> {
    let mut value = 0_u64;
    for (index, byte) in data.iter().copied().enumerate().take(10) {
        let shift = index * 7;
        let payload = u64::from(byte & 0x7f);
        if shift == 63 && payload > 1 {
            break;
        }
        value |= payload << shift;
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(format!(
        "invalid ULEB128 stack size at byte {section_offset}"
    ))
}

fn compact_function(frame: &StackFrame) -> String {
    const LIMIT: usize = 180;
    let name = frame
        .functions
        .first()
        .map(String::as_str)
        .unwrap_or("<unknown>");
    let mut end = name
        .char_indices()
        .nth(LIMIT)
        .map_or(name.len(), |(index, _)| index);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    if end == name.len() {
        name.to_owned()
    } else {
        format!("{}…", &name[..end])
    }
}

fn compact_source(frame: &StackFrame) -> String {
    let Some(source) = &frame.source else {
        return String::new();
    };
    match (source.line, source.column) {
        (Some(line), Some(column)) => format!("  [{}:{line}:{column}]", source.file),
        (Some(line), None) => format!("  [{}:{line}]", source.file),
        _ => format!("  [{}]", source.file),
    }
}

fn bytes(value: u64) -> String {
    if value >= 1024 * 1024 {
        format!("{:.2} MiB", value as f64 / (1024.0 * 1024.0))
    } else if value >= 1024 {
        format!("{:.2} KiB", value as f64 / 1024.0)
    } else {
        format!("{value} B")
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use object::{
        Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
        write::{Object, Symbol, SymbolSection},
    };

    use super::{
        ReviewedStackFrame, StackBudget, StackFrame, StackSourceLocation, analyze_stack,
        audit_stack, decode_stack_sizes, reviewed_frame_matches,
    };

    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    fn budget() -> StackBudget {
        StackBudget {
            schema: 3,
            stack_start_symbol: "_stack_start".into(),
            stack_end_symbol: "_stack_end".into(),
            warn_frame_bytes: 8 * 1024,
            max_frame_bytes: 32 * 1024,
            max_move_bytes: 4 * 1024,
            runtime_cpu0_minimum_free_bytes: 32 * 1024,
            runtime_cpu1_minimum_free_bytes: 4 * 1024,
            reported_frame_count: 30,
            reviewed_frames: vec![ReviewedStackFrame {
                function_contains: "<unknown function at".into(),
                source_ends_with: Vec::new(),
                max_bytes: 32 * 1024,
                reason: "synthetic fixture".into(),
            }],
        }
    }

    #[test]
    fn stack_policy_separates_review_and_hard_limits() {
        assert!(budget().validate().is_ok());
        let mut invalid = budget();
        invalid.warn_frame_bytes = invalid.max_frame_bytes + 1;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn decodes_little_endian_32_bit_stack_entries() {
        let data = [
            0x88, 0x08, 0x00, 0x2f, 0x00, // 0x2f000888, 0 bytes
            0xf0, 0x08, 0x00, 0x2f, 0xd0, 0x01, // 0x2f0008f0, 208 bytes
        ];
        assert_eq!(
            decode_stack_sizes(&data, 4, true).unwrap(),
            vec![(0x2f00_0888, 0), (0x2f00_08f0, 208)]
        );
    }

    #[test]
    fn rejects_truncated_stack_metadata() {
        assert!(decode_stack_sizes(&[1, 2, 3], 4, true).is_err());
        assert!(decode_stack_sizes(&[1, 0, 0, 0, 0x80], 4, true).is_err());
    }

    #[test]
    fn frame_json_retains_complete_function_identity() {
        let frame = StackFrame {
            address: 0x5000_0000,
            size: 123,
            functions: vec!["crate::complete::function".into()],
            source: None,
        };
        assert_eq!(frame.functions[0], "crate::complete::function");
    }

    #[test]
    fn exact_frame_limit_passes_and_one_byte_over_fails() {
        let at_limit = test_elf(true, 32 * 1024, 0x2000, 0x1000);
        let report = analyze_stack(&at_limit, &budget()).unwrap();
        assert!(report.violations.is_empty());
        assert!(audit_stack(&report).is_ok());
        fs::remove_file(at_limit).unwrap();

        let over_limit = test_elf(true, 32 * 1024 + 1, 0x2000, 0x1000);
        let report = analyze_stack(&over_limit, &budget()).unwrap();
        assert_eq!(report.violations.len(), 1);
        assert!(audit_stack(&report).is_err());
        fs::remove_file(over_limit).unwrap();
    }

    #[test]
    fn unreviewed_large_frame_and_reviewed_growth_fail() {
        let path = test_elf(true, 9 * 1024, 0x2000, 0x1000);
        let mut policy = budget();
        policy.reviewed_frames.clear();
        let report = analyze_stack(&path, &policy).unwrap();
        assert!(report.audit.errors[0].contains("unreviewed frame"));
        assert!(audit_stack(&report).is_err());

        policy.reviewed_frames.push(ReviewedStackFrame {
            function_contains: "<unknown function at".into(),
            source_ends_with: Vec::new(),
            max_bytes: 8 * 1024 + 512,
            reason: "synthetic fixture".into(),
        });
        let report = analyze_stack(&path, &policy).unwrap();
        assert!(report.audit.errors[0].contains("reviewed frame"));
        assert!(audit_stack(&report).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reviewed_source_accepts_repository_and_cargo_trimmed_identities() {
        let reviewed = ReviewedStackFrame {
            function_contains: "::supervisor::run".into(),
            source_ends_with: vec![
                "driver/integration/esp32s31/embassy-wifi/src/supervisor.rs".into(),
                "open-esp-radio-esp32s31-embassy-wifi-0.1.0/src/supervisor.rs".into(),
            ],
            max_bytes: 16 * 1024,
            reason: "fixture".into(),
        };
        let frame = |file: &str| StackFrame {
            address: 0,
            size: 9 * 1024,
            functions: vec!["crate::supervisor::run::{closure#0}".into()],
            source: Some(StackSourceLocation {
                file: file.into(),
                line: Some(1),
                column: Some(1),
            }),
        };

        assert!(reviewed_frame_matches(
            &frame("/checkout/driver/integration/esp32s31/embassy-wifi/src/supervisor.rs"),
            &reviewed
        ));
        assert!(reviewed_frame_matches(
            &frame("./open-esp-radio-esp32s31-embassy-wifi-0.1.0/src/supervisor.rs"),
            &reviewed
        ));
        assert!(!reviewed_frame_matches(
            &frame("./another-package-0.1.0/src/supervisor.rs"),
            &reviewed
        ));
    }

    #[test]
    fn missing_metadata_and_invalid_stack_symbols_fail_closed() {
        let missing = test_elf(false, 0, 0x2000, 0x1000);
        let error = analyze_stack(&missing, &budget()).unwrap_err().to_string();
        assert!(error.contains("missing .stack_sizes"));
        fs::remove_file(missing).unwrap();

        let invalid = test_elf(true, 64, 0x1000, 0x2000);
        let error = analyze_stack(&invalid, &budget()).unwrap_err().to_string();
        assert!(error.contains("empty or upward-growing range"));
        fs::remove_file(invalid).unwrap();
    }

    fn test_elf(
        include_stack_sizes: bool,
        frame_size: u64,
        stack_start: u64,
        stack_end: u64,
    ) -> std::path::PathBuf {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        for (name, value, kind) in [
            ("_stack_start", stack_start, SymbolKind::Data),
            ("_stack_end", stack_end, SymbolKind::Data),
            ("fixture_poll", 0x3000, SymbolKind::Text),
        ] {
            object.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value,
                size: 0,
                kind,
                scope: SymbolScope::Linkage,
                weak: false,
                section: SymbolSection::Absolute,
                flags: SymbolFlags::None,
            });
        }
        if include_stack_sizes {
            let section = object.add_section(
                Vec::new(),
                b".stack_sizes".to_vec(),
                SectionKind::ReadOnlyData,
            );
            let mut entry = 0x3000_u32.to_le_bytes().to_vec();
            encode_uleb128(frame_size, &mut entry);
            object.append_section_data(section, &entry, 1);
        }
        let path = std::env::temp_dir().join(format!(
            "open-radio-stack-{}-{}.elf",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, object.write().unwrap()).unwrap();
        path
    }

    fn encode_uleb128(mut value: u64, output: &mut Vec<u8>) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            output.push(byte);
            if value == 0 {
                return;
            }
        }
    }
}
