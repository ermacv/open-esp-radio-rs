//! Projection of reviewed semantic actions into event-dispatch facts.

use super::super::*;

pub(in crate::analysis::linked_ir) fn project_event_dispatches(
    actions: &[LinkedProjectedSemanticAction],
) -> Vec<LinkedEventDispatch> {
    actions
        .iter()
        .enumerate()
        .filter_map(|(semantic_action_index, action)| {
            let spec = action.contract.as_ref()?.event_dispatch.as_ref()?;
            let mut blockers = BTreeSet::new();
            if spec.mechanism.is_empty() {
                blockers.insert("event dispatch mechanism is empty".to_owned());
            }
            if spec.execution_context.is_empty() {
                blockers.insert("event dispatch execution context is empty".to_owned());
            }
            let expected_names = spec
                .argument_roles
                .iter()
                .map(|binding| binding.argument)
                .collect::<BTreeSet<_>>();
            for argument in &action.arguments {
                if !expected_names.contains(argument.name.as_str()) {
                    blockers.insert(format!(
                        "unexpected semantic argument {} at position {}",
                        argument.name, argument.position
                    ));
                }
            }
            let mut bindings = Vec::new();
            let mut declared_roles = BTreeSet::new();
            let mut declared_arguments = BTreeSet::new();
            for binding in &spec.argument_roles {
                let role = binding.role;
                let name = binding.argument;
                if !declared_roles.insert(role) {
                    blockers.insert(format!("duplicate event role {role}"));
                }
                if !declared_arguments.insert(name) {
                    blockers.insert(format!("duplicate event argument {name}"));
                }
                if role.is_empty() {
                    blockers.insert(format!("semantic argument {name} has an empty event role"));
                }
                if name.is_empty() {
                    blockers.insert(format!("event role {role} has an empty semantic argument"));
                }
                let matching = action
                    .arguments
                    .iter()
                    .filter(|argument| argument.name == name)
                    .collect::<Vec<_>>();
                match matching.as_slice() {
                    [argument] => bindings.push(LinkedEventDispatchBinding {
                        role,
                        argument: (*argument).clone(),
                    }),
                    [] => {
                        blockers
                            .insert(format!("missing semantic argument {name} for role {role}"));
                    }
                    _ => {
                        blockers.insert(format!(
                            "ambiguous semantic argument {name} for role {role}"
                        ));
                    }
                }
            }
            Some(LinkedEventDispatch {
                semantic_action_index,
                mechanism: spec.mechanism,
                execution_context: spec.execution_context,
                receiver: spec.receiver.map(str::to_owned),
                interface_complete: blockers.is_empty(),
                blockers: blockers.into_iter().collect(),
                bindings,
            })
        })
        .collect()
}
