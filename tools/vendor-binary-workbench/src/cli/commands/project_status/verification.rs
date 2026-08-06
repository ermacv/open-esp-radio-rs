//! Public verification-policy and accepted-evidence readiness.

use std::path::Path;

use super::{
    super::ProjectContext,
    model::{Component, Phase, Readiness},
};
use crate::{profiles, verification::dispositions, verification::load_evidence_baseline};

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    Phase::collect(
        "verification",
        vec![
            profile_pack(context.target.profiles.as_deref()),
            disposition_pack(context.target.dispositions.as_deref()),
            evidence_baseline(context.target.evidence_baseline.as_deref()),
        ],
    )
}

fn profile_pack(path: Option<&Path>) -> Component {
    let Some(path) = path else {
        return Component::new("profiles", Readiness::NotConfigured);
    };
    if !path.is_file() {
        return Component::new("profiles", Readiness::Incomplete)
            .detail("path", path.display().to_string())
            .diagnostic("verification profile pack is missing");
    }
    match profiles::load(path) {
        Ok(profiles) => Component::new("profiles", Readiness::Ready)
            .detail("path", path.display().to_string())
            .detail("contracts", profiles.len()),
        Err(error) => Component::new("profiles", Readiness::Invalid)
            .detail("path", path.display().to_string())
            .diagnostic(error),
    }
}

fn disposition_pack(path: Option<&Path>) -> Component {
    let Some(path) = path else {
        return Component::new("dispositions", Readiness::NotConfigured);
    };
    if !path.is_file() {
        return Component::new("dispositions", Readiness::Incomplete)
            .detail("path", path.display().to_string())
            .diagnostic("verification disposition pack is missing");
    }
    match dispositions::Manifest::load(path) {
        Ok(manifest) => Component::new("dispositions", Readiness::Ready)
            .detail("path", path.display().to_string())
            .detail("entries", manifest.entries().count()),
        Err(error) => Component::new("dispositions", Readiness::Invalid)
            .detail("path", path.display().to_string())
            .diagnostic(error),
    }
}

fn evidence_baseline(path: Option<&Path>) -> Component {
    let Some(path) = path else {
        return Component::new("evidence_baseline", Readiness::NotConfigured);
    };
    if !path.is_file() {
        return Component::new("evidence_baseline", Readiness::Incomplete)
            .detail("path", path.display().to_string())
            .diagnostic("accepted evidence baseline is missing");
    }
    match load_evidence_baseline(path) {
        Ok(evidence) => Component::new("evidence_baseline", Readiness::Ready)
            .detail("path", path.display().to_string())
            .detail("entries", evidence.len())
            .detail("review_command", "verify evidence"),
        Err(error) => Component::new("evidence_baseline", Readiness::Invalid)
            .detail("path", path.display().to_string())
            .diagnostic(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TargetSpec;

    #[test]
    fn checked_target_exposes_parseable_public_verification_inputs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workbench remains under tools");
        let target =
            TargetSpec::load(&root.join("verification/vendor/targets/esp32s31/target.spec"))
                .unwrap();
        for component in [
            profile_pack(target.profiles.as_deref()),
            disposition_pack(target.dispositions.as_deref()),
            evidence_baseline(target.evidence_baseline.as_deref()),
        ] {
            assert_eq!(component.status, Readiness::Ready, "{component:?}");
        }
    }
}
