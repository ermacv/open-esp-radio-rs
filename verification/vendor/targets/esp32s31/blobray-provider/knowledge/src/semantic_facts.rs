//! Reviewed meaning of an exact private event boundary; no execution algorithm.

use open_radio_vendor_semantics::*;

const PP_POST_ARGUMENTS: &[ExternalArgumentSpec] = &[ExternalArgumentSpec {
    name: "signal",
    c_type: "u32",
    direction: ExternalArgumentDirection::Input,
}];
pub const PP_POST_EVENT_ROLES: &[SemanticArgumentRoleSpec] = &[SemanticArgumentRoleSpec {
    role: "selector",
    argument: "signal",
}];

pub static PP_POST_SEMANTIC: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "esp32s31-libpp-pp-post-v1",
    source: "esp32s31-reviewed-knowledge",
    c_name: "pp_post",
    argument_count: 1,
    body_policy: SemanticFunctionBodyPolicy::AnalyzeBody,
    return_model: ExternalReturnModel::Unmodeled,
    semantic: ExternalSemanticSpec {
        operation: "wifi.internal-signal.post",
        arguments: PP_POST_ARGUMENTS,
        return_type: "void",
        replacement: Some("typed Rust ISR-to-radio-owner signal"),
        event_dispatch: Some(EventDispatchSemanticSpec {
            mechanism: "internal-signal",
            execution_context: "unspecified",
            receiver: Some("esp32s31::pp-task"),
            argument_roles: PP_POST_EVENT_ROLES,
        }),
    },
    evidence: "exact-body-and-relocation-schema",
};
