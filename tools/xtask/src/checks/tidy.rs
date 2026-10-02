//! The fast integrity tier: `oer-tidy`, a small package without Cargo or
//! compiler work of its own, checked before anything builds.

use crate::{Context, Result};
use oer_process as process;

/// Run every `oer-tidy` check over this checkout.
pub fn run(ctx: &Context) -> Result<()> {
    process::run(
        ctx.cargo()
            .args(["run", "--quiet", "-p", "oer-tidy", "--", "check", "--root"])
            .arg(&ctx.root),
    )
}
