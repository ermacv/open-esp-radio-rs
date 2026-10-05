//! Every host package declares its application owner and exposed boundary.

use oer_repo::{HostApp, HostBoundary, HostLayer};

use crate::Context;

pub fn check(context: &Context<'_>) -> Vec<String> {
    let mut problems = Vec::new();
    for package in context.model.packages() {
        let Ok(class) = context.model.classification(package) else {
            continue;
        };
        if class.host_layer.is_none() {
            continue;
        }
        let valid = match (class.host_app, class.host_boundary) {
            (Some(HostApp::Formats), boundary) => boundary == Some(HostBoundary::Format),
            (
                Some(HostApp::Foundation | HostApp::Devices | HostApp::Images | HostApp::Analysis),
                boundary,
            ) => boundary == Some(HostBoundary::Library),
            (Some(_), Some(HostBoundary::Application | HostBoundary::Library)) => true,
            _ => false,
        };
        if !valid {
            problems.push(format!(
                "{}: a host package needs host-app and host-boundary; shared formats use formats/format",
                package.manifest
            ));
        }
        if class.host_layer == Some(HostLayer::Entry)
            && class.host_boundary != Some(HostBoundary::Application)
        {
            problems.push(format!(
                "{}: a command line needs host-boundary = \"application\"",
                package.manifest
            ));
        }
    }
    problems.sort();
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::problems;

    #[test]
    fn ownership_is_required_and_command_lines_cannot_be_shared_libraries() {
        let manifest = |extra| {
            format!(
                "[package]\nname = \"oer-cli\"\n[package.metadata.open-radio]\nlayer = \"tool\"\nplatform = \"host\"\nhost-layer = \"entry\"\n{extra}"
            )
        };
        assert!(!problems(&[("cli/Cargo.toml", &manifest(""))], check).is_empty());
        assert!(
            !problems(
                &[(
                    "cli/Cargo.toml",
                    &manifest("host-app = \"stand\"\nhost-boundary = \"library\"")
                )],
                check
            )
            .is_empty()
        );
        assert!(
            problems(
                &[(
                    "cli/Cargo.toml",
                    &manifest("host-app = \"stand\"\nhost-boundary = \"application\"")
                )],
                check
            )
            .is_empty()
        );
    }
}
