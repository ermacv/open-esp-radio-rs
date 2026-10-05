//! `cargo xtask check changed`: what this checkout changed against its
//! merge base with a base revision, committed or not, checked by the
//! registry's fast checks that the change selects ([`crate::registry`]);
//! with `--full`, also its full-tier checks, as CI runs them on every push.

use std::collections::BTreeSet;

use crate::{
    Result, gate,
    registry::{self, Tier},
};
use oer_process::Checkout;

pub fn run(ctx: &Checkout, base: &str, full: bool) -> Result<()> {
    let merge_base = gate::merge_base(ctx, base)?;
    let mut files: BTreeSet<String> = gate::committed(ctx, &merge_base)?.into_iter().collect();
    files.extend(gate::uncommitted(ctx)?);
    let tier = if full { Tier::Full } else { Tier::Fast };
    let change = gate::Change::of(ctx, files.into_iter().collect(), &merge_base, None, tier)?;
    println!(
        "check changed: {} files against {base}; {} package(s) changed, {} with dependents",
        change.files.len(),
        change.selection.packages.len(),
        change.affected.len()
    );
    registry::run_change(ctx, &change)?;
    crate::ci_status::print(ctx, "check changed");
    println!(
        "check changed passed{}",
        if full {
            ""
        } else {
            "; `--full` adds what CI checks on the pull request"
        }
    );
    Ok(())
}
