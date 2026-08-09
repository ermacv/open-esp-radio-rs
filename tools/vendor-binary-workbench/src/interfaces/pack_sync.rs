//! Conservative synchronization of generated observations into an editable pack.

use std::{collections::BTreeSet, fs, path::Path};

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

use super::{InterfaceFactRoot, InterfaceFactStep, InterfaceFacts, PackOrigin, ReviewStatus};
use crate::Result;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub(crate) struct InterfacePackSyncSummary {
    pub(crate) added_anchors: usize,
    pub(crate) refreshed_anchors: usize,
    pub(crate) removed_anchors: usize,
    pub(crate) added_slots: usize,
    pub(crate) removed_slots: usize,
}

impl InterfacePackSyncSummary {
    pub(crate) const fn changed(self) -> bool {
        self.added_anchors != 0
            || self.refreshed_anchors != 0
            || self.removed_anchors != 0
            || self.added_slots != 0
            || self.removed_slots != 0
    }
}

pub(crate) fn sync_interface_pack(
    path: &Path,
    facts: &InterfaceFacts,
    calling_convention: &str,
    check: bool,
) -> Result<InterfacePackSyncSummary> {
    let input = fs::read_to_string(path)
        .map_err(|error| crate::Error::read("interface pack", path, error))?;
    let mut document = input.parse::<DocumentMut>().map_err(|error| {
        crate::error::WorkbenchError::manifest_source(
            "interface pack",
            path,
            &input,
            &error,
            error.span(),
        )
    })?;
    if document.get("schema").and_then(Item::as_integer) != Some(1) {
        return Err(crate::Error::invalid(format!(
            "interface pack {} requires schema = 1",
            path.display()
        )));
    }
    let pack = super::pack_parse::parse(&document)?;
    if pack.calling_convention != calling_convention {
        return Err(crate::Error::invalid(format!(
            "interface pack calling convention {:?} does not match project target {:?}",
            pack.calling_convention, calling_convention
        )));
    }
    let anchors = document
        .get_mut("anchors")
        .and_then(Item::as_array_of_tables_mut)
        .ok_or("interface pack has no [[anchors]] array")
        .map_err(crate::Error::invalid)?;
    if anchors.len() != pack.anchors.len() {
        return Err(crate::Error::invalid(
            "interface pack parser/document anchor count mismatch",
        ));
    }

    let mut summary = InterfacePackSyncSummary::default();
    let mut matched_facts = BTreeSet::new();
    let mut remove_anchors = Vec::new();
    for (anchor_index, anchor) in pack.anchors.iter().enumerate() {
        let matches = facts
            .tables
            .iter()
            .enumerate()
            .filter(|(_, fact)| super::pack::anchor_matches_without_digest(anchor, facts, fact))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if anchor.origin == PackOrigin::Manual && !matches.is_empty() {
            return Err(crate::Error::invalid(format!(
                "manual interface anchor {:?} now matches generated facts; review it before synchronization",
                anchor.id
            )));
        }
        matched_facts.extend(matches.iter().copied());
        if matches.is_empty() {
            if anchor.origin == PackOrigin::Observed && anchor.status == ReviewStatus::Unreviewed {
                remove_anchors.push(anchor_index);
                summary.removed_anchors += 1;
            } else if anchor.origin == PackOrigin::Observed {
                return Err(crate::Error::invalid(format!(
                    "protected interface anchor {:?} is stale; synchronization cannot remove reviewed or ignored evidence",
                    anchor.id
                )));
            }
            continue;
        }
        if anchor.status != ReviewStatus::Unreviewed
            && !matches.iter().any(|index| {
                super::pack::anchor_digest_matches(anchor, facts, &facts.tables[*index])
            })
        {
            return Err(crate::Error::invalid(format!(
                "protected interface anchor {:?} has a stale artifact guard; review it before synchronization",
                anchor.id
            )));
        }
        if anchor.status == ReviewStatus::Ignored {
            continue;
        }
        let observed = matches
            .iter()
            .flat_map(|index| facts.tables[*index].slots.iter())
            .filter(|slot| slot.selector.is_none())
            .map(|slot| (slot.offset, slot.width))
            .collect::<BTreeSet<_>>();
        if let Some(slot) = anchor.slots.iter().find(|slot| {
            slot.origin == PackOrigin::Manual && observed.contains(&(slot.offset, slot.width))
        }) {
            return Err(crate::Error::invalid(format!(
                "manual interface slot {:+#x}/{} in anchor {:?} is now observed; review its origin before synchronization",
                slot.offset, slot.width, anchor.id
            )));
        }
        if let Some(stale) = anchor.slots.iter().find(|slot| {
            slot.origin == PackOrigin::Observed
                && slot.status != ReviewStatus::Unreviewed
                && !observed.contains(&(slot.offset, slot.width))
        }) {
            return Err(crate::Error::invalid(format!(
                "protected interface slot {:+#x}/{} in anchor {:?} is stale; synchronization cannot remove reviewed or ignored evidence",
                stale.offset, stale.width, anchor.id
            )));
        }
        if anchor.status == ReviewStatus::Reviewed {
            let present = anchor
                .slots
                .iter()
                .map(|slot| (slot.offset, slot.width))
                .collect::<BTreeSet<_>>();
            for key in observed.iter().filter(|key| !present.contains(key)) {
                validate_new_observation_against_reviewed_layout(anchor, *key)?;
            }
        }
        let table = anchors
            .get_mut(anchor_index)
            .expect("document and parsed anchors have equal lengths");
        let pointer_width = table
            .get("pointer-width")
            .and_then(Item::as_integer)
            .and_then(|width| u32::try_from(width).ok())
            .unwrap_or(32);
        let layout_size = {
            let slots = ensure_array_of_tables(table, "slots");
            let before = slots.len();
            slots.retain(|slot| {
                let removable = table_string(slot, "origin") == Some("observed")
                    && table_string(slot, "status") == Some("unreviewed");
                let key = table_slot_key(slot);
                !removable || key.is_some_and(|key| observed.contains(&key))
            });
            summary.removed_slots += before - slots.len();

            let mut present = slots
                .iter()
                .filter_map(table_slot_key)
                .collect::<BTreeSet<_>>();
            for key in observed {
                if present.insert(key) {
                    insert_slot_sorted(slots, new_unreviewed_slot(key.0, key.1));
                    summary.added_slots += 1;
                }
            }
            unreviewed_layout_size(pointer_width, slots)
        };
        if anchor.status == ReviewStatus::Unreviewed {
            let old_layout_size = table.get("layout-size").and_then(Item::as_integer);
            let layout_size = i64::from(layout_size);
            table.insert("layout-size", value(layout_size));
            let metadata_changed = old_layout_size != Some(layout_size)
                || update_unreviewed_digest(table, facts, &matches);
            summary.refreshed_anchors += usize::from(metadata_changed);
        }
    }
    for index in remove_anchors.iter().copied().rev() {
        anchors.remove(index);
    }

    let mut ids = pack
        .anchors
        .iter()
        .enumerate()
        .filter(|(index, _)| !remove_anchors.contains(index))
        .map(|(_, anchor)| anchor.id.clone())
        .collect::<BTreeSet<_>>();
    for (fact_index, fact) in facts.tables.iter().enumerate() {
        if matched_facts.contains(&fact_index) {
            continue;
        }
        let artifact = facts
            .artifact(fact.artifact)
            .expect("validated interface facts reference an artifact");
        if artifact.sources.len() != 1 {
            return Err(crate::Error::invalid(format!(
                "artifact {} has {} source identities; cannot synchronize an interface anchor",
                artifact.index,
                artifact.sources.len()
            )));
        }
        let source = artifact.sources.iter().next().expect("one source");
        let root_name = match &fact.root {
            InterfaceFactRoot::RelocatedSymbol { symbol, .. } => symbol.clone(),
            InterfaceFactRoot::FunctionArgument { argument } => format!("arg{argument}"),
            InterfaceFactRoot::AbsoluteAddress { address } => format!("address_{address:08x}"),
        };
        let base = super::pack_template::identifier_from(&format!("{source}.{root_name}"));
        let id = unique_id(&base, &mut ids);
        anchors.push(new_unreviewed_anchor(
            &id,
            source,
            fact,
            artifact.sha256.as_deref(),
        ));
        summary.added_anchors += 1;
        summary.added_slots += fact
            .slots
            .iter()
            .filter(|slot| slot.selector.is_none())
            .count();
    }

    let rendered = document.to_string();
    if !summary.changed() {
        return Ok(summary);
    }
    if check {
        return Err(crate::Error::invalid(format!(
            "interface pack differs from current observations in {}; rerun interfaces sync-pack without --check",
            path.display()
        )));
    }
    write_atomic(path, &rendered)?;
    Ok(summary)
}

fn validate_new_observation_against_reviewed_layout(
    anchor: &super::InterfaceAnchor,
    (offset, width): (i32, u8),
) -> Result<()> {
    let observed_offset = offset;
    let Some(pointer_width) = anchor.pointer_width else {
        return reviewed_layout_error(anchor, observed_offset, width);
    };
    let Some(layout_size) = anchor.layout_size else {
        return reviewed_layout_error(anchor, observed_offset, width);
    };
    let Some(stride) = anchor
        .slot_stride
        .map(u32::from)
        .filter(|stride| *stride != 0)
    else {
        return reviewed_layout_error(anchor, observed_offset, width);
    };
    let offset = u32::try_from(offset).ok();
    let end = offset.and_then(|offset| offset.checked_add(u32::from(width) / 8));
    if width != pointer_width
        || offset.is_none_or(|offset| offset % stride != 0)
        || end.is_none_or(|end| end > layout_size)
    {
        return reviewed_layout_error(anchor, observed_offset, width);
    }
    Ok(())
}

fn reviewed_layout_error<T>(anchor: &super::InterfaceAnchor, offset: i32, width: u8) -> Result<T> {
    Err(crate::Error::invalid(format!(
        "new observed interface slot {offset:+#x}/{width} does not fit the reviewed layout of anchor {:?}; review the layout before synchronization",
        anchor.id
    )))
}

fn new_unreviewed_anchor(
    id: &str,
    source: &str,
    fact: &super::InterfaceTableFact,
    sha256: Option<&str>,
) -> Table {
    let mut table = Table::new();
    table.insert("id", value(id));
    table.insert("status", value("unreviewed"));
    table.insert("origin", value("observed"));
    table.insert("source", value(source));
    write_root(&mut table, &fact.root);
    table.insert(
        "container-path",
        Item::Value(Value::Array(steps(&fact.container_path))),
    );
    table.insert("layout-version", value("unreviewed"));
    let pointer_width = fact.slots.iter().map(|slot| slot.width).max().unwrap_or(32);
    table.insert("pointer-width", value(i64::from(pointer_width)));
    let pointer_bytes = u32::from(pointer_width) / 8;
    let layout_size = fact
        .slots
        .iter()
        .filter(|slot| slot.selector.is_none())
        .filter_map(|slot| u32::try_from(slot.offset).ok())
        .filter_map(|offset| offset.checked_add(pointer_bytes))
        .max()
        .unwrap_or(pointer_bytes);
    table.insert("layout-size", value(i64::from(layout_size)));
    table.insert("slot-stride", value(i64::from(pointer_bytes)));
    if let Some(sha256) = sha256 {
        let mut guards = ArrayOfTables::new();
        let mut guard = Table::new();
        guard.insert("kind", value("artifact-sha256"));
        guard.insert("sha256", value(sha256));
        guards.push(guard);
        table.insert("guards", Item::ArrayOfTables(guards));
    }
    let mut slots = ArrayOfTables::new();
    for slot in fact.slots.iter().filter(|slot| slot.selector.is_none()) {
        slots.push(new_unreviewed_slot(slot.offset, slot.width));
    }
    table.insert("slots", Item::ArrayOfTables(slots));
    table
}

fn write_root(table: &mut Table, root: &InterfaceFactRoot) {
    table.insert("root-kind", value(root.kind()));
    match root {
        InterfaceFactRoot::RelocatedSymbol {
            member,
            symbol,
            addend,
            addressing,
        } => {
            table.insert("symbol", value(symbol));
            if let Some(member) = member {
                table.insert("member", value(member));
            }
            table.insert("addend", value(*addend));
            table.insert("addressing", value(addressing));
        }
        InterfaceFactRoot::FunctionArgument { argument } => {
            table.insert("argument", value(i64::from(*argument)));
        }
        InterfaceFactRoot::AbsoluteAddress { address } => {
            table.insert("address", value(i64::from(*address)));
        }
    }
}

fn steps(values: &[InterfaceFactStep]) -> Array {
    let mut output = Array::new();
    for step in values {
        let mut item = InlineTable::new();
        item.insert("offset", Value::from(i64::from(step.offset)));
        item.insert("width", Value::from(i64::from(step.width)));
        output.push(Value::InlineTable(item));
    }
    output
}

fn new_unreviewed_slot(offset: i32, width: u8) -> Table {
    let mut slot = Table::new();
    slot.insert("offset", value(i64::from(offset)));
    slot.insert("width", value(i64::from(width)));
    slot.insert("status", value("unreviewed"));
    slot.insert("origin", value("observed"));
    slot
}

fn insert_slot_sorted(slots: &mut ArrayOfTables, slot: Table) {
    let key = table_slot_key(&slot).expect("generated slot has a key");
    let index = slots
        .iter()
        .position(|current| table_slot_key(current).is_some_and(|current| current > key))
        .unwrap_or_else(|| slots.len());
    slots.insert(index, slot);
}

fn unreviewed_layout_size(pointer_width: u32, slots: &ArrayOfTables) -> u32 {
    let bytes = pointer_width / 8;
    slots
        .iter()
        .filter_map(table_slot_key)
        .filter_map(|(offset, _)| u32::try_from(offset).ok())
        .filter_map(|offset| offset.checked_add(bytes))
        .max()
        .unwrap_or(bytes)
}

fn update_unreviewed_digest(anchor: &mut Table, facts: &InterfaceFacts, matches: &[usize]) -> bool {
    let digests = matches
        .iter()
        .filter_map(|index| facts.artifact(facts.tables[*index].artifact))
        .filter_map(|artifact| artifact.sha256.as_deref())
        .collect::<BTreeSet<_>>();
    let Some(digest) = digests
        .iter()
        .copied()
        .next()
        .filter(|_| digests.len() == 1)
    else {
        return false;
    };
    let Some(guards) = anchor
        .get_mut("guards")
        .and_then(Item::as_array_of_tables_mut)
    else {
        return false;
    };
    let mut changed = false;
    for guard in guards.iter_mut() {
        if table_string(guard, "kind") == Some("artifact-sha256") {
            if table_string(guard, "sha256") != Some(digest) {
                guard.insert("sha256", value(digest));
                changed = true;
            }
        }
    }
    changed
}

fn table_slot_key(table: &Table) -> Option<(i32, u8)> {
    Some((
        i32::try_from(table.get("offset")?.as_integer()?).ok()?,
        u8::try_from(table.get("width")?.as_integer()?).ok()?,
    ))
}

fn table_string<'a>(table: &'a Table, key: &str) -> Option<&'a str> {
    table.get(key).and_then(Item::as_str)
}

fn ensure_array_of_tables<'a>(table: &'a mut Table, key: &str) -> &'a mut ArrayOfTables {
    if !table.contains_key(key) {
        table.insert(key, Item::ArrayOfTables(ArrayOfTables::new()));
    }
    table
        .get_mut(key)
        .and_then(Item::as_array_of_tables_mut)
        .expect("validated pack uses an array of tables")
}

fn unique_id(base: &str, ids: &mut BTreeSet<String>) -> String {
    let mut id = base.to_owned();
    let mut suffix = 2;
    while !ids.insert(id.clone()) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    id
}

fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("interface pack path must have a UTF-8 file name")
        .map_err(crate::Error::invalid)?;
    let staging = parent.join(format!(
        ".{name}.vendor-workbench-sync-{}",
        std::process::id()
    ));
    if staging.exists() {
        return Err(crate::Error::invalid(format!(
            "interface pack staging path exists: {}",
            staging.display()
        )));
    }
    fs::write(&staging, contents)?;
    if let Err(error) = fs::rename(&staging, path) {
        let _ = fs::remove_file(&staging);
        return Err(error.into());
    }
    Ok(())
}
