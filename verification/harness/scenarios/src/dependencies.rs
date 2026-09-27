//! What a scenario's verdicts depend on in its compiled production probe.
//!
//! Three layers: the repository source files of every executed production
//! instruction (from its line information, inlined frames included), the
//! declaration file of every named data object the execution read, and the
//! content of every probe data range the execution read. Files outside the
//! repository (the standard library, registry crates) are pinned by the
//! toolchain and the lock files, which the evidence records globally. A
//! dependency that cannot be attributed makes the scenario fall back to its
//! probe's whole source closure, stated in the result.
use crate::harness::{Result, invalid};
use object::{Object, ObjectSection, ObjectSymbol, SectionKind, SymbolKind};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The dependencies of one scenario's execution of its probe image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dependencies {
    /// Repository-relative source files executed code or read named data
    /// come from.
    pub files: BTreeSet<PathBuf>,
    /// SHA-256 of the sorted multiset of the probe data ranges the
    /// execution read, independent of their addresses.
    pub read_data: String,
    /// Why the files cannot stand for the execution, when they cannot.
    pub fallback: Option<String>,
}

type Dwarf<'a> = gimli::Dwarf<gimli::EndianSlice<'a, gimli::RunTimeEndian>>;

fn load_dwarf<'a>(file: &object::File<'a>) -> Result<Dwarf<'a>> {
    let endian = gimli::RunTimeEndian::Little;
    Ok(gimli::Dwarf::load(|id| {
        let data = file
            .section_by_name(id.name())
            .and_then(|section| section.data().ok())
            .unwrap_or(&[]);
        Ok::<_, gimli::Error>(gimli::EndianSlice::new(data, endian))
    })?)
}

/// `path` relative to the canonical repository `root`, when inside it.
fn relative(path: &Path, root: &Path) -> Option<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let canonical = absolute.canonicalize().ok()?;
    canonical.strip_prefix(root).ok().map(Path::to_path_buf)
}

/// The declaration file of every data variable DWARF places at a fixed
/// address, by that address.
fn declarations(dwarf: &Dwarf<'_>) -> Result<BTreeMap<u64, PathBuf>> {
    let mut files = BTreeMap::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs()? {
            if entry.tag() != gimli::DW_TAG_variable {
                continue;
            }
            let Some(gimli::AttributeValue::Exprloc(expression)) =
                entry.attr_value(gimli::DW_AT_location)
            else {
                continue;
            };
            let mut operations = expression.operations(unit.encoding());
            let Ok(Some(gimli::Operation::Address { address })) = operations.next() else {
                continue;
            };
            let Some(gimli::AttributeValue::FileIndex(index)) =
                entry.attr_value(gimli::DW_AT_decl_file)
            else {
                continue;
            };
            let Some(program) = &unit.line_program else {
                continue;
            };
            let header = program.header();
            let Some(file) = header.file(index) else {
                continue;
            };
            let mut path = PathBuf::new();
            if let Some(directory) = file.directory(header) {
                path.push(
                    dwarf
                        .attr_string(&unit, directory)?
                        .to_string_lossy()
                        .as_ref(),
                );
            }
            path.push(
                dwarf
                    .attr_string(&unit, file.path_name())?
                    .to_string_lossy()
                    .as_ref(),
            );
            files.insert(address, path);
        }
    }
    Ok(files)
}

/// Contiguous byte ranges covering `reads`, as `[start, end)`.
fn merge(reads: &BTreeSet<(u32, u8)>) -> Vec<(u64, u64)> {
    let mut ranges: Vec<(u64, u64)> = vec![];
    for &(address, width) in reads {
        let (start, end) = (u64::from(address), u64::from(address) + u64::from(width));
        match ranges.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => ranges.push((start, end)),
        }
    }
    ranges
}

/// The repository files of `executed` instructions, whose source paths
/// `locate` returns, and the first instruction without any source line.
fn attribute(
    executed: &[u32],
    root: &Path,
    mut locate: impl FnMut(u32) -> Result<Vec<PathBuf>>,
) -> Result<(BTreeSet<PathBuf>, Option<u32>)> {
    let mut files = BTreeSet::new();
    let mut unattributed = None;
    for &pc in executed {
        let paths = locate(pc)?;
        if paths.is_empty() {
            unattributed.get_or_insert(pc);
        }
        files.extend(paths.iter().filter_map(|path| relative(path, root)));
    }
    Ok((files, unattributed))
}

/// SHA-256 of `regions` as a sorted multiset: each length-prefixed.
pub fn content_digest(mut regions: Vec<Vec<u8>>) -> String {
    regions.sort();
    let mut hash = Sha256::new();
    for region in &regions {
        hash.update((region.len() as u64).to_le_bytes());
        hash.update(region);
    }
    format!("{:x}", hash.finalize())
}

/// The dependencies of `executed` instructions and `reads` of ordinary
/// memory in probe image `elf`, attributed within repository `root`.
pub fn of(
    elf: &[u8],
    executed: &BTreeSet<u32>,
    reads: &BTreeSet<(u32, u8)>,
    root: &Path,
) -> Result<Dependencies> {
    let root = root.canonicalize()?;
    let file = object::File::parse(elf)?;
    let dwarf = load_dwarf(&file)?;
    let mut result = Dependencies::default();
    let mut fallback = |reason: String| {
        if result.fallback.is_none() {
            result.fallback = Some(reason);
        }
    };
    let context = addr2line::Context::from_dwarf(load_dwarf(&file)?)?;
    // Only the probe's own code: the executions also run the linked vendor
    // image, whose instructions are not production's.
    let text: Vec<(u64, u64)> = file
        .sections()
        .filter(|s| s.kind() == SectionKind::Text)
        .map(|s| (s.address(), s.address() + s.size()))
        .collect();
    let probe: Vec<u32> = executed
        .iter()
        .copied()
        .filter(|pc| {
            text.iter()
                .any(|(s, e)| *s <= u64::from(*pc) && u64::from(*pc) < *e)
        })
        .collect();
    let (files, unattributed) = attribute(&probe, &root, |pc| {
        let mut paths = vec![];
        let mut frames = context
            .find_frames(u64::from(pc))
            .skip_all_loads()
            .map_err(|e| invalid(format!("debug information at {pc:#x}: {e}")))?;
        while let Some(frame) = frames
            .next()
            .map_err(|e| invalid(format!("debug information at {pc:#x}: {e}")))?
        {
            if let Some(path) = frame.location.and_then(|l| l.file) {
                paths.push(PathBuf::from(path));
            }
        }
        Ok(paths)
    })?;
    result.files = files;
    if let Some(pc) = unattributed {
        fallback(format!("executed instruction {pc:#x} has no source line"));
    }
    let declarations = declarations(&dwarf)?;
    let named: Vec<(u64, u64, String)> = file
        .symbols()
        .filter(|s| s.kind() == SymbolKind::Data && s.size() > 0)
        // Compiler-generated anonymous constants carry no declaration: the
        // content digest stands for them.
        .filter(|s| {
            !s.name()
                .is_ok_and(|n| n.starts_with(".L") || n.starts_with("anon."))
        })
        .map(|s| {
            (
                s.address(),
                s.address() + s.size(),
                s.name().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let data: Vec<(u64, u64, &[u8])> = file
        .sections()
        .filter(|s| {
            matches!(
                s.kind(),
                SectionKind::ReadOnlyData | SectionKind::ReadOnlyString | SectionKind::Data
            )
        })
        .filter_map(|s| Some((s.address(), s.address() + s.size(), s.data().ok()?)))
        .collect();
    let mut regions = vec![];
    for (start, end) in merge(reads) {
        let Some((base, _, bytes)) = data.iter().find(|(s, e, _)| *s <= start && end <= *e) else {
            continue;
        };
        let offset = (start - base) as usize;
        let Some(region) = bytes.get(offset..offset + (end - start) as usize) else {
            continue;
        };
        regions.push(region.to_vec());
        for (symbol_start, symbol_end, name) in &named {
            if *symbol_start < end && start < *symbol_end {
                match declarations.get(symbol_start) {
                    Some(path) => {
                        if let Some(path) = relative(path, &root) {
                            result.files.insert(path);
                        }
                    }
                    None => fallback(format!("read data object {name} has no declaration file")),
                }
            }
        }
    }
    result.read_data = content_digest(regions);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_read_data_digest_depends_on_content_not_placement() {
        let a = content_digest(vec![vec![1, 2], vec![3]]);
        assert_eq!(a, content_digest(vec![vec![3], vec![1, 2]]));
        // A constant table another file defines changes only its bytes.
        assert_ne!(a, content_digest(vec![vec![1, 9], vec![3]]));
        assert_ne!(a, content_digest(vec![vec![1], vec![2, 3]]));
    }

    #[test]
    fn an_instruction_without_a_source_line_is_reported_for_fallback() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("crates/a")).unwrap();
        std::fs::write(root.join("crates/a/lib.rs"), "").unwrap();
        let (files, unattributed) = attribute(&[0x10, 0x14, 0x18], &root, |pc| {
            Ok(match pc {
                0x10 => vec![root.join("crates/a/lib.rs")],
                // A registry crate outside the repository.
                0x14 => vec![PathBuf::from("/cargo/registry/core/src/lib.rs")],
                _ => vec![],
            })
        })
        .unwrap();
        assert_eq!(files, BTreeSet::from([PathBuf::from("crates/a/lib.rs")]));
        assert_eq!(unattributed, Some(0x18));
    }

    #[test]
    fn overlapping_and_adjacent_reads_merge() {
        let reads = BTreeSet::from([(0x10, 4), (0x12, 4), (0x16, 2), (0x20, 1)]);
        assert_eq!(merge(&reads), [(0x10, 0x18), (0x20, 0x21)]);
    }
}
