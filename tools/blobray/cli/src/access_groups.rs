//! Selection and grouped presentation of `register-accesses` records.
//!
//! A filter selects observations only. Blocked functions and gaps stay in the
//! output and the summary always counts the whole analysis, so a selection
//! cannot hide that the analysis was incomplete.
use blobray_domain::{RegisterAccess, RegisterMask, RegisterMaskKind};
use oer_riscv_model::{FunctionRecord, MemoryKind, ObjectLocation, SymbolId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which observations a `register-accesses` request keeps.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccessFilter {
    /// Word addresses; an observation is kept when its address lies in one of
    /// these 32-bit words. Empty keeps every address, unresolved included.
    pub words: Vec<u32>,
    /// Function names; empty keeps every function.
    pub functions: Vec<String>,
}

impl AccessFilter {
    /// Whether `record` belongs in the output: every blocked function and gap,
    /// and each observation the filter selects.
    pub fn keeps(&self, record: &RegisterAccess) -> bool {
        match record {
            RegisterAccess::Observation {
                function, address, ..
            } => {
                let word = self.words.is_empty()
                    || address.is_some_and(|address| self.words.contains(&(address & !3)));
                word && self.names(function.name.as_deref())
            }
            RegisterAccess::Blocked { function, .. } => self.names(function.name.as_deref()),
            RegisterAccess::Gap { .. } => true,
        }
    }

    fn names(&self, name: Option<&[u8]>) -> bool {
        self.functions.is_empty()
            || name.is_some_and(|name| {
                self.functions
                    .iter()
                    .any(|wanted| wanted.as_bytes() == name)
            })
    }
}

/// The key observations are grouped by.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroupBy {
    /// One group per accessed 32-bit word, with the functions accessing it.
    Address,
    /// One group per function, with the words it accesses.
    Function,
}

/// One group of observations sharing an address or a function.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessGroup {
    /// The 32-bit word every observation of the group lies in; `None` groups
    /// unresolved addresses (address grouping) or is absent (function grouping).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word: Option<u32>,
    /// The function, as `input` and name; absent for address grouping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<GroupFunction>,
    pub entries: Vec<AccessEntry>,
}

/// A function by its input position, display name and symbol identity.
///
/// The symbol keeps functions apart that share a name: local functions of
/// different archive members, unnamed functions and names equal only after
/// lossy UTF-8 decoding. Groups order by input, then name, then symbol.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupFunction {
    pub input: u64,
    /// The symbol name, lossily decoded for display; empty when unnamed.
    pub name: String,
    pub symbol: SymbolId,
}

impl GroupFunction {
    /// `name (input 0, member 3)`; `<unnamed>` for a function without a name
    /// and no member for a standalone ELF.
    pub fn human(&self) -> String {
        let name = if self.name.is_empty() {
            "<unnamed>"
        } else {
            &self.name
        };
        match self.symbol.object.location {
            ObjectLocation::ArchiveMember { ordinal } => {
                format!("{name} (input {}, member {ordinal})", self.input)
            }
            _ => format!("{name} (input {})", self.input),
        }
    }
}

/// Observations of one group that agree on the other key, access and mask.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessEntry {
    /// The accessed word, for function grouping; `None` there is unresolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word: Option<u32>,
    /// The accessing function, for address grouping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<GroupFunction>,
    /// `load`, `store`, `load-reserved`, `store-conditional`, `atomic`,
    /// `float-load`, `float-store`; `expression` for a masked load/store
    /// expression; `other` for any other fact.
    pub access: String,
    pub width: Option<u8>,
    pub mask: Option<RegisterMask>,
    pub count: u64,
}

/// A group: its word or its function.
type GroupKey = (Option<u32>, Option<GroupFunction>);
/// A group's entries with each entry's mask and count.
type Entries = BTreeMap<EntryKey, (Option<RegisterMask>, u64)>;
type EntryKey = (
    Option<u32>,
    Option<GroupFunction>,
    String,
    Option<u8>,
    Option<(u8, u32)>,
);

/// Aggregates selected observations into groups in key order.
#[derive(Clone, Debug)]
pub struct AccessGroups {
    by: GroupBy,
    groups: BTreeMap<GroupKey, Entries>,
}

impl AccessGroups {
    pub fn new(by: GroupBy) -> Self {
        Self {
            by,
            groups: BTreeMap::new(),
        }
    }

    /// Count `record` when it is an observation; other records are not grouped.
    pub fn add(&mut self, record: &RegisterAccess) {
        let RegisterAccess::Observation {
            function,
            fact,
            address,
            mask,
            ..
        } = record
        else {
            return;
        };
        let function = GroupFunction {
            input: function.input,
            name: function
                .name
                .as_deref()
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .unwrap_or_default(),
            symbol: function.symbol.clone(),
        };
        let word = address.map(|address| address & !3);
        let (access, width) = access(fact);
        let mask_key = mask.map(|mask| (mask_kind(mask.kind), mask.bits));
        let (group, entry) = match self.by {
            GroupBy::Address => (
                (word, None),
                (None, Some(function), access, width, mask_key),
            ),
            GroupBy::Function => (
                (None, Some(function)),
                (word, None, access, width, mask_key),
            ),
        };
        let slot = self
            .groups
            .entry(group)
            .or_default()
            .entry(entry)
            .or_insert((*mask, 0));
        slot.1 += 1;
    }

    /// The groups in key order: addresses ascending with unresolved first, or
    /// functions by input and name.
    pub fn groups(&self) -> Vec<AccessGroup> {
        self.groups
            .iter()
            .map(|((word, function), entries)| AccessGroup {
                word: *word,
                function: function.clone(),
                entries: entries
                    .iter()
                    .map(
                        |((word, function, access, width, _), (mask, count))| AccessEntry {
                            word: *word,
                            function: function.clone(),
                            access: access.clone(),
                            width: *width,
                            mask: *mask,
                            count: *count,
                        },
                    )
                    .collect(),
            })
            .collect()
    }

    /// One header line per group and one indented line per entry.
    pub fn human(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for group in self.groups() {
            lines.push(match (&group.function, group.word) {
                (Some(function), _) => function.human(),
                (None, Some(word)) => format!("{word:#010x}"),
                (None, None) => String::from("unresolved"),
            });
            for entry in &group.entries {
                let subject = match (&entry.function, entry.word) {
                    (Some(function), _) => function.human(),
                    (None, Some(word)) => format!("{word:#010x}"),
                    (None, None) => String::from("unresolved"),
                };
                let width = entry
                    .width
                    .map(|width| format!(" {width}B"))
                    .unwrap_or_default();
                let mask = entry
                    .mask
                    .map(|mask| match mask.kind {
                        RegisterMaskKind::ReadSelection => format!(" read {:#010x}", mask.bits),
                        RegisterMaskKind::WriteReplacement => {
                            format!(" replace {:#010x}", mask.bits)
                        }
                    })
                    .unwrap_or_default();
                lines.push(format!(
                    "  {subject} {}{width}{mask} x{}",
                    entry.access, entry.count
                ));
            }
        }
        lines
    }
}

fn mask_kind(kind: RegisterMaskKind) -> u8 {
    match kind {
        RegisterMaskKind::ReadSelection => 0,
        RegisterMaskKind::WriteReplacement => 1,
    }
}

fn access(fact: &FunctionRecord) -> (String, Option<u8>) {
    match fact {
        FunctionRecord::MemoryAccess { access, width, .. } => {
            let name = match access {
                MemoryKind::Load => "load",
                MemoryKind::Store => "store",
                MemoryKind::LoadReserved => "load-reserved",
                MemoryKind::StoreConditional => "store-conditional",
                MemoryKind::Atomic => "atomic",
                MemoryKind::FloatLoad => "float-load",
                MemoryKind::FloatStore => "float-store",
            };
            (name.into(), Some(*width))
        }
        FunctionRecord::Expression { .. } => ("expression".into(), None),
        _ => ("other".into(), None),
    }
}

#[cfg(test)]
mod tests;
