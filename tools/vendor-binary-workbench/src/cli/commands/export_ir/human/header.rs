//! Project and input-artifact section.

use std::fmt::Write as _;

use super::super::*;

pub(super) fn render(
    mut output: &mut String,
    artifacts: &[IrArtifactInput],
    report: &LinkedIrReport,
    include_reachable: bool,
) {
    let _ = writeln!(
        &mut output,
        "PROJECT\tlinkage={}\tcall-linkage={}\tselection={}\tcall-compaction=stable-identity-universal-affine-bindings\tdiagnostic-compaction=exact-semicolon-fragment-inventory\tcontext-projection=affine-simple-call-paths\treturn-provenance=exact-bit-ranges-with-constant-and-unknown-masks\tsemantic-actions=lexical-site-paths-factorized-cfg-guards-affine-root-bindings\tevent-dispatch=reviewed-contract-declared-role-projection\tevent-dispatch-effect-completeness-claim=false\tevent-dispatch-receiver-inference=none\tevent-dispatch-receiver-source=reviewed-contract-or-unknown\tcfg-guards=forced-branch-paths-minimized-dnf-factorized-by-function\tcfg-guard-expressions=pseudo-rust-aligned-bit-masks-with-symbolic-fallback\tcfg-guard-result-sources=bit-provenance-with-operand-comparison-mapping-and-producer-targets\tcfg-guard-mmio-linkage=recursive-exact-bit-projection-with-producer-paths\tdirect-mmio-predicates=exact-bit-provenance-with-constant-comparison-mapping\tsemantic-field-guards=action-identity-and-path-coordinate-preserving\tdirect-mmio-predicate-completeness-claim=false\tscenario-suggestions=structural-candidates-require-concrete-replay\tscenario-suggestion-proof-claim=false\tmmio-field-candidates=contiguous-subregister-write-poll-and-direct-guard-evidence\tmmio-field-semantics-claim=false\tcfg-guard-completeness-claim=false\tartifacts={}",
        if artifacts.len() > 1 {
            "independent-artifacts"
        } else {
            "primary-with-companions"
        },
        if artifacts.len() > 1 {
            "unique-exported-symbol-only"
        } else {
            "primary-resolver"
        },
        if include_reachable {
            "symbol-prefix-with-reachable-internal-callees"
        } else {
            "symbol-prefix-only"
        },
        artifacts.len()
    );
    for artifact in artifacts {
        let functions = report
            .functions
            .iter()
            .filter(|function| function.source == artifact.source)
            .count();
        let _ = writeln!(
            &mut output,
            "ARTIFACT\t{}\t{}\tfunctions={}",
            artifact.source,
            artifact.path.display(),
            functions
        );
    }
}
