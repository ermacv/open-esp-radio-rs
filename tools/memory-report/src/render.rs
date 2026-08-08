use std::collections::BTreeMap;

use serde::Serialize;

use crate::{AuditReport, MemoryReport};

#[derive(Clone, Debug, Serialize)]
pub struct MemoryDiff {
    pub schema: u32,
    pub before: String,
    pub after: String,
    pub regions: Vec<RegionDiff>,
    pub consumers: Vec<ConsumerDiff>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegionDiff {
    pub id: String,
    pub allocated_before: u64,
    pub allocated_after: u64,
    pub allocated_delta: i128,
    pub free_before: u64,
    pub free_after: u64,
    pub free_delta: i128,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConsumerDiff {
    pub owner: String,
    pub region_before: Option<String>,
    pub region_after: Option<String>,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub bytes_delta: i128,
}

pub fn diff(before: &MemoryReport, after: &MemoryReport) -> MemoryDiff {
    let mut region_ids = before
        .regions
        .iter()
        .chain(&after.regions)
        .map(|region| region.id.clone())
        .collect::<Vec<_>>();
    region_ids.sort();
    region_ids.dedup();
    let regions = region_ids
        .into_iter()
        .map(|id| {
            let old = before.regions.iter().find(|region| region.id == id);
            let new = after.regions.iter().find(|region| region.id == id);
            let allocated_before = old.map_or(0, |region| region.allocated);
            let allocated_after = new.map_or(0, |region| region.allocated);
            let free_before = old.map_or(0, |region| region.free);
            let free_after = new.map_or(0, |region| region.free);
            RegionDiff {
                id,
                allocated_before,
                allocated_after,
                allocated_delta: delta(allocated_before, allocated_after),
                free_before,
                free_after,
                free_delta: delta(free_before, free_after),
            }
        })
        .collect();

    let mut owners = BTreeMap::<
        String,
        (
            Option<&crate::ConsumerReport>,
            Option<&crate::ConsumerReport>,
        ),
    >::new();
    for consumer in &before.consumers {
        owners.entry(consumer.owner.clone()).or_default().0 = Some(consumer);
    }
    for consumer in &after.consumers {
        owners.entry(consumer.owner.clone()).or_default().1 = Some(consumer);
    }
    let mut consumers = owners
        .into_iter()
        .map(|(owner, (old, new))| {
            let bytes_before = old.map_or(0, |consumer| consumer.bytes);
            let bytes_after = new.map_or(0, |consumer| consumer.bytes);
            ConsumerDiff {
                owner,
                region_before: old.map(|consumer| consumer.region.clone()),
                region_after: new.map(|consumer| consumer.region.clone()),
                bytes_before,
                bytes_after,
                bytes_delta: delta(bytes_before, bytes_after),
            }
        })
        .collect::<Vec<_>>();
    consumers.sort_by_key(|consumer| std::cmp::Reverse(consumer.bytes_delta.unsigned_abs()));

    MemoryDiff {
        schema: 1,
        before: before.elf.display().to_string(),
        after: after.elf.display().to_string(),
        regions,
        consumers,
    }
}

pub fn render_report(report: &MemoryReport) -> String {
    let mut output = format!("Memory report: {}\n\n", report.elf.display());
    for region in &report.regions {
        output.push_str(&format!(
            "{} ({:?}) {:#010x}..{:#010x}\n",
            region.id, region.kind, region.start, region.end
        ));
        output.push_str(&format!(
            "  capacity={} allocated={} reserved={} free={}\n",
            bytes(region.capacity),
            bytes(region.allocated),
            bytes(region.reserved),
            bytes(region.free)
        ));
        output.push_str(&format!(
            "  executable={} read-only={} mutable={} policy-attributed={} mutable-unattributed={}\n",
            bytes(region.executable),
            bytes(region.read_only),
            bytes(region.mutable),
            bytes(region.policy_attributed),
            bytes(region.mutable_unattributed)
        ));
        for reserve in &region.reservations {
            output.push_str(&format!(
                "  reserve {:<26} {:>12}  {:?}\n",
                reserve.id,
                bytes(reserve.size),
                reserve.reason
            ));
        }
        output.push('\n');
    }

    output.push_str("Policy-attributed consumers\n");
    for consumer in &report.consumers {
        let geometry = match (consumer.declared_count, consumer.element_capacity) {
            (Some(count), Some(capacity)) => format!(" {count}x{capacity}"),
            (Some(count), None) => format!(" count={count}"),
            _ => String::new(),
        };
        output.push_str(&format!(
            "  {:<34} {:>12}  {:<14} {:?}/{:?}{}\n",
            consumer.owner,
            bytes(consumer.bytes),
            consumer.region,
            consumer.reason,
            consumer.placement,
            geometry
        ));
        if let Some(optimization) = &consumer.optimization {
            output.push_str(&format!("    optimization: {optimization}\n"));
        }
    }

    if !report.largest_unclassified.is_empty() {
        output.push_str("\nLargest unclassified symbols\n");
        for allocation in &report.largest_unclassified {
            output.push_str(&format!(
                "  {:>12}  {:<14} {}\n",
                bytes(allocation.size),
                allocation.region.as_deref().unwrap_or("<outside>"),
                allocation.demangled
            ));
        }
    }
    output.push('\n');
    output.push_str(&render_audit(&report.audit));
    output
}

pub fn render_audit(audit: &AuditReport) -> String {
    let mut output = format!(
        "Audit: {} error(s), {} warning(s)\n",
        audit.errors.len(),
        audit.warnings.len()
    );
    for error in &audit.errors {
        output.push_str(&format!("  ERROR: {error}\n"));
    }
    for warning in &audit.warnings {
        output.push_str(&format!("  WARNING: {warning}\n"));
    }
    output
}

pub fn render_diff(report: &MemoryDiff) -> String {
    let mut output = format!(
        "Memory diff\n  before={}\n  after={}\n\n",
        report.before, report.after
    );
    output.push_str("Regions\n");
    for region in &report.regions {
        output.push_str(&format!(
            "  {:<16} allocated {} -> {} ({})  free {} -> {} ({})\n",
            region.id,
            bytes(region.allocated_before),
            bytes(region.allocated_after),
            signed_bytes(region.allocated_delta),
            bytes(region.free_before),
            bytes(region.free_after),
            signed_bytes(region.free_delta)
        ));
    }
    output.push_str("\nConsumers\n");
    for consumer in &report.consumers {
        if consumer.bytes_delta == 0 && consumer.region_before == consumer.region_after {
            continue;
        }
        output.push_str(&format!(
            "  {:<34} {} -> {} ({})  {:?} -> {:?}\n",
            consumer.owner,
            bytes(consumer.bytes_before),
            bytes(consumer.bytes_after),
            signed_bytes(consumer.bytes_delta),
            consumer.region_before,
            consumer.region_after
        ));
    }
    output
}

fn delta(before: u64, after: u64) -> i128 {
    i128::from(after) - i128::from(before)
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

fn signed_bytes(value: i128) -> String {
    let sign = if value >= 0 { "+" } else { "-" };
    let magnitude = value.unsigned_abs();
    if magnitude >= 1024 * 1024 {
        format!("{sign}{:.2} MiB", magnitude as f64 / (1024.0 * 1024.0))
    } else if magnitude >= 1024 {
        format!("{sign}{:.2} KiB", magnitude as f64 / 1024.0)
    } else {
        format!("{sign}{magnitude} B")
    }
}
