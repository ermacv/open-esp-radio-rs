//! Public verification-policy and accepted-evidence readiness.

use std::path::{Path, PathBuf};

use super::model::{Component, Phase, Readiness};
use crate::application::ProjectContext;
use crate::{profiles, verification::dispositions, verification::load_evidence_baseline};

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    let project_profiles = context
        .project
        .verification
        .as_ref()
        .map(|workspace| workspace.profiles.iter().map(PathBuf::as_path).collect());
    let target_profiles = context
        .target
        .profiles
        .as_deref()
        .map(|path| vec![path])
        .unwrap_or_default();
    Phase::collect(
        "verification",
        vec![
            profile_packs(project_profiles.unwrap_or(target_profiles)),
            disposition_pack(context.target.dispositions.as_deref()),
            evidence_baseline(context.target.evidence_baseline.as_deref()),
        ],
    )
}

fn profile_packs(paths: Vec<&Path>) -> Component {
    if paths.is_empty() {
        return Component::new("profiles", Readiness::NotConfigured);
    }
    let display = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut contracts = 0usize;
    for path in &paths {
        if !path.is_file() {
            return Component::new("profiles", Readiness::Incomplete)
                .detail("paths", display)
                .diagnostic(format!(
                    "verification profile pack {} is missing",
                    path.display()
                ));
        }
        match profiles::load(path) {
            Ok(loaded) => contracts += loaded.len(),
            Err(error) => {
                return Component::new("profiles", Readiness::Invalid)
                    .detail("paths", display)
                    .diagnostic(error);
            }
        }
    }
    Component::new("profiles", Readiness::Ready)
        .detail("paths", display)
        .detail("packs", paths.len())
        .detail("contracts", contracts)
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
            TargetSpec::load(&root.join("verification/vendor/targets/esp32s31/target.toml"))
                .unwrap();
        for component in [
            profile_packs(
                target
                    .profiles
                    .as_deref()
                    .map(|path| vec![path])
                    .unwrap_or_default(),
            ),
            disposition_pack(target.dispositions.as_deref()),
            evidence_baseline(target.evidence_baseline.as_deref()),
        ] {
            assert_eq!(component.status, Readiness::Ready, "{component:?}");
        }
    }
}
