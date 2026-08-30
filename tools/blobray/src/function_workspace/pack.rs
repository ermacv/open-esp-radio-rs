//! Editable function/context pack and its resolved workspace view.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use toml_edit::{Document, DocumentMut, Item};

use super::FunctionFacts;
use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FunctionReviewStatus {
    Reviewed,
    Ignored,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedFunctionInput {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedContextField {
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) status: FunctionReviewStatus,
    pub(crate) name: Option<String>,
    pub(crate) display_type: Option<String>,
    pub(crate) description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedContext {
    pub(crate) argument: u8,
    pub(crate) status: FunctionReviewStatus,
    pub(crate) name: Option<String>,
    pub(crate) type_name: Option<String>,
    pub(crate) fields: Vec<ReviewedContextField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedFunction {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) status: FunctionReviewStatus,
    pub(crate) name: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) signature: Option<ReviewedFunctionSignature>,
    pub(crate) accept_incomplete: bool,
    pub(crate) preconditions: Vec<ReviewedPrecondition>,
    pub(crate) paths: Vec<ReviewedPath>,
    pub(crate) contexts: Vec<ReviewedContext>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedFunctionSignature {
    pub(crate) arguments: Vec<ReviewedFunctionArgument>,
    pub(crate) return_abi: Option<String>,
    pub(crate) return_role: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedFunctionArgument {
    pub(crate) index: u8,
    pub(crate) name: String,
    pub(crate) abi: String,
    pub(crate) role: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedPrecondition {
    pub(crate) id: String,
    pub(crate) expression: String,
    pub(crate) rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedPath {
    pub(crate) id: String,
    pub(crate) class: String,
    pub(crate) summary: String,
    pub(crate) evidence: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReviewedEventRoute {
    SelectorDelivery(ReviewedSelectorEventRoute),
    StaticEventCallback(ReviewedStaticEventCallbackRoute),
    BrokerSubscription(ReviewedBrokerSubscriptionRoute),
}

impl ReviewedEventRoute {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::SelectorDelivery(route) => &route.id,
            Self::StaticEventCallback(route) => &route.id,
            Self::BrokerSubscription(route) => &route.id,
        }
    }

    pub(crate) fn replay(&self) -> Option<&ReviewedEventReplay> {
        match self {
            Self::SelectorDelivery(route) => route.replay.as_ref(),
            Self::StaticEventCallback(_) | Self::BrokerSubscription(_) => None,
        }
    }

    fn exact_fact_functions(&self, functions: &mut BTreeMap<String, BTreeSet<String>>) {
        let mut require = |profile: &str, identity: &str| {
            functions
                .entry(profile.to_owned())
                .or_default()
                .insert(identity.to_owned());
        };
        match self {
            Self::SelectorDelivery(_) => {}
            Self::StaticEventCallback(route) => {
                require(&route.profile, &route.dispatcher);
                require(&route.binding_profile, &route.binding_entry);
                require(&route.delivery_profile, &route.delivery_entry);
                require(&route.callback_profile, &route.callback_function);
            }
            Self::BrokerSubscription(route) => {
                require(&route.profile, &route.dispatcher);
                require(&route.domain.profile, &route.domain.entry);
                require(&route.binding_profile, &route.binding_entry);
                require(&route.callback_profile, &route.callback_function);
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedSelectorEventRoute {
    pub(crate) id: String,
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) dispatcher: String,
    pub(crate) mechanism: String,
    pub(crate) selector_role: String,
    pub(crate) selector_value: u32,
    pub(crate) receiver: Option<String>,
    pub(crate) execution_context: String,
    pub(crate) consumer_profile: String,
    pub(crate) consumer_source: String,
    pub(crate) consumer_entry: String,
    pub(crate) delivery: ReviewedEventDelivery,
    pub(crate) case_handler: Option<ReviewedEventCaseHandler>,
    pub(crate) terminal: Option<ReviewedEventTerminal>,
    pub(crate) replay: Option<ReviewedEventReplay>,
    pub(crate) rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedStaticEventCallbackRoute {
    pub(crate) id: String,
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) dispatcher: String,
    pub(crate) mechanism: String,
    pub(crate) execution_context: String,
    pub(crate) dispatch_call: ReviewedEventCallMatcher,
    pub(crate) dispatch_sites: Vec<u32>,
    pub(crate) upstream_chain: Vec<String>,
    pub(crate) upstream_sites: Vec<u32>,
    pub(crate) dispatch_object_argument: u8,
    pub(crate) dispatch_queue_argument: u8,
    pub(crate) binding_profile: String,
    pub(crate) binding_source: String,
    pub(crate) binding_entry: String,
    pub(crate) binding_call: ReviewedEventCallMatcher,
    pub(crate) binding_site: u32,
    pub(crate) binding_object_argument: u8,
    pub(crate) binding_callback_argument: u8,
    pub(crate) delivery_profile: String,
    pub(crate) delivery_source: String,
    pub(crate) delivery_entry: String,
    pub(crate) receive_call: ReviewedEventCallMatcher,
    pub(crate) receive_site: u32,
    pub(crate) receive_queue_argument: u8,
    pub(crate) run_call: ReviewedEventCallMatcher,
    pub(crate) run_site: u32,
    pub(crate) run_event_argument: u8,
    pub(crate) callback_profile: String,
    pub(crate) callback_source: String,
    pub(crate) callback_function: String,
    pub(crate) rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedBrokerSubscriptionRoute {
    pub(crate) id: String,
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) dispatcher: String,
    pub(crate) mechanism: String,
    pub(crate) execution_context: String,
    pub(crate) dispatch_call: ReviewedEventCallMatcher,
    pub(crate) dispatch_site: u32,
    pub(crate) dispatch_selector_argument: u8,
    pub(crate) selector_role: String,
    pub(crate) selector_value: u32,
    pub(crate) dispatch_payload_argument: u8,
    pub(crate) payload_role: String,
    pub(crate) payload_value: String,
    pub(crate) domain: ReviewedEventDomainWitness,
    pub(crate) binding_profile: String,
    pub(crate) binding_source: String,
    pub(crate) binding_entry: String,
    pub(crate) binding_call: ReviewedEventCallMatcher,
    pub(crate) binding_site: u32,
    pub(crate) binding_domain_argument: u8,
    pub(crate) binding_object_argument: u8,
    pub(crate) binding_callback_store_site: u32,
    pub(crate) binding_callback_store_offset: i64,
    pub(crate) callback_profile: String,
    pub(crate) callback_source: String,
    pub(crate) callback_function: String,
    pub(crate) callback_selector_argument: u8,
    pub(crate) case_handler: ReviewedEventCaseHandler,
    pub(crate) case_handler_site: u32,
    pub(crate) terminal: Option<ReviewedEventTerminal>,
    pub(crate) rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedEventDomainWitness {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) entry: String,
    pub(crate) call: ReviewedEventCallMatcher,
    pub(crate) call_site: u32,
    pub(crate) dispatch_argument: u8,
    pub(crate) call_object_argument: u8,
    pub(crate) call_selector_argument: u8,
    pub(crate) selector_value: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReviewedEventCallMatcher {
    Operation(String),
    Function(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedEventReplay {
    pub(crate) manifest: PathBuf,
    pub(crate) source: String,
    pub(crate) evidence: PathBuf,
    pub(crate) producer_phase: String,
    pub(crate) consumer_phase: String,
    pub(crate) state_observation: String,
    pub(crate) state_model: ReviewedEventStateModel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReviewedEventStateModel {
    CountedLatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedEventDelivery {
    pub(crate) operation: String,
    pub(crate) output_role: String,
    pub(crate) selector_offset: u32,
    pub(crate) selector_width: u8,
    pub(crate) encoding: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedEventCaseHandler {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) function: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedEventTerminal {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) function: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ReviewedMemoryObject {
    Argument {
        function: String,
        index: u8,
    },
    Global {
        member: Option<String>,
        symbol: String,
    },
    Dereferenced {
        pointer: Box<ReviewedMemoryObject>,
        pointer_offset: i64,
    },
    Absolute {
        address_space: String,
        address: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedTypeBinding {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) name: String,
    pub(crate) object: ReviewedMemoryObject,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedTypeField {
    pub(crate) offset: i64,
    pub(crate) width: u8,
    pub(crate) status: FunctionReviewStatus,
    pub(crate) name: Option<String>,
    pub(crate) display_type: Option<String>,
    pub(crate) description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedLogicalType {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) bindings: Vec<ReviewedTypeBinding>,
    pub(crate) fields: Vec<ReviewedTypeField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionPack {
    pub(crate) id: String,
    pub(crate) inputs: Vec<ReviewedFunctionInput>,
    pub(crate) functions: Vec<ReviewedFunction>,
    pub(crate) types: Vec<ReviewedLogicalType>,
    pub(crate) event_routes: Vec<ReviewedEventRoute>,
}

struct LoadedFunctionPack {
    value: FunctionPack,
    input: String,
    document: Document<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct FunctionWorkspaceSummary {
    pub(crate) inputs: usize,
    pub(crate) observed_functions: usize,
    pub(crate) reviewed_functions: usize,
    pub(crate) ignored_functions: usize,
    pub(crate) unreviewed_functions: usize,
    pub(crate) reviewed_contexts: usize,
    pub(crate) ignored_contexts: usize,
    pub(crate) unreviewed_contexts: usize,
    pub(crate) reviewed_fields: usize,
    pub(crate) ignored_fields: usize,
    pub(crate) unreviewed_fields: usize,
    pub(crate) accepted_incomplete: usize,
    pub(crate) logical_types: usize,
    pub(crate) type_bindings: usize,
    pub(crate) type_fields: usize,
    pub(crate) reviewed_type_fields: usize,
    pub(crate) ignored_type_fields: usize,
    pub(crate) unreviewed_type_fields: usize,
    pub(crate) event_routes: usize,
}

#[derive(Debug)]
pub(crate) struct FunctionWorkspace {
    pub(crate) facts: FunctionFacts,
    pub(crate) pack: FunctionPack,
    summary: FunctionWorkspaceSummary,
}

impl FunctionWorkspace {
    pub(crate) fn load(reports: &[(String, std::path::PathBuf)], pack_path: &Path) -> Result<Self> {
        let facts = FunctionFacts::load(reports)?;
        let pack = FunctionPack::load(pack_path)?;
        let summary = super::pack_validate::validate(&pack.value, &facts).map_err(|error| {
            crate::error::BlobrayError::manifest_source(
                "function pack",
                pack_path,
                &pack.input,
                &error,
                error.span(&pack.document),
            )
        })?;
        Ok(Self {
            facts,
            pack: pack.value,
            summary,
        })
    }

    pub(crate) fn load_with_callback_facts(
        reports: &[(String, std::path::PathBuf)],
        pack_path: &Path,
    ) -> Result<Self> {
        let pack = FunctionPack::load(pack_path)?;
        let mut exact_functions = BTreeMap::new();
        for route in &pack.value.event_routes {
            route.exact_fact_functions(&mut exact_functions);
        }
        let facts = FunctionFacts::load_with_functions(reports, &exact_functions)?;
        let summary = super::pack_validate::validate(&pack.value, &facts).map_err(|error| {
            crate::error::BlobrayError::manifest_source(
                "function pack",
                pack_path,
                &pack.input,
                &error,
                error.span(&pack.document),
            )
        })?;
        Ok(Self {
            facts,
            pack: pack.value,
            summary,
        })
    }

    pub(crate) fn load_summary(
        reports: &[(String, std::path::PathBuf)],
        pack_path: &Path,
    ) -> Result<Self> {
        let facts = FunctionFacts::load_summary(reports)?;
        let pack = FunctionPack::load(pack_path)?;
        let summary =
            super::pack_validate::validate_summary(&pack.value, &facts).map_err(|error| {
                crate::error::BlobrayError::manifest_source(
                    "function pack",
                    pack_path,
                    &pack.input,
                    &error,
                    error.span(&pack.document),
                )
            })?;
        Ok(Self {
            facts,
            pack: pack.value,
            summary,
        })
    }

    pub(crate) const fn summary(&self) -> FunctionWorkspaceSummary {
        self.summary
    }
}

impl FunctionPack {
    pub(crate) fn load_reviewed(path: &Path) -> Result<Self> {
        Ok(Self::load(path)?.value)
    }

    #[tracing::instrument(name = "load_function_pack", fields(path = %path.display()))]
    fn load(path: &Path) -> Result<LoadedFunctionPack> {
        let input = fs::read_to_string(path)
            .map_err(|error| crate::Error::read("function pack", path, error))?;
        let source_document = Document::parse(input.clone()).map_err(|error| {
            crate::error::BlobrayError::manifest_source(
                "function pack",
                path,
                &input,
                &error,
                error.span(),
            )
        })?;
        let document: DocumentMut = source_document.clone().into_mut();
        if document.get("schema").and_then(Item::as_integer) != Some(11) {
            return Err(crate::error::BlobrayError::manifest_source(
                "function pack",
                path,
                &input,
                "requires schema = 11",
                source_document.get("schema").and_then(Item::span),
            ));
        }
        let mut value = super::pack_parse::parse(&document)
            .map_err(|error| crate::error::BlobrayError::manifest("function pack", path, error))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        for route in &mut value.event_routes {
            if let ReviewedEventRoute::SelectorDelivery(route) = route
                && let Some(replay) = &mut route.replay
            {
                if replay.manifest.is_relative() {
                    replay.manifest = base.join(&replay.manifest);
                }
                if replay.evidence.is_relative() {
                    replay.evidence = base.join(&replay.evidence);
                }
            }
        }
        Ok(LoadedFunctionPack {
            value,
            input,
            document: source_document,
        })
    }
}
