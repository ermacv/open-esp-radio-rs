//! Typed ownership and completion routes for linked-IR blockers.
//!
//! A route names a project file only when Blobray already consumes the
//! corresponding record. Analyzer limitations must never be presented as a
//! TOML editing task merely because a project-local pack exists.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{ProjectSpec, Result};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockerResolutionOwner {
    GenericBackend,
    AnalysisAddon,
    ProjectFunctionFact,
    InterfacePack,
    RuntimeScenario,
    VerificationDisposition,
    ReviewedKnowledge,
    ProjectComposition,
    Unsupported,
}

impl BlockerResolutionOwner {
    pub const fn label(self) -> &'static str {
        match self {
            Self::GenericBackend => "generic-backend",
            Self::AnalysisAddon => "analysis-addon",
            Self::ProjectFunctionFact => "project-function-fact",
            Self::InterfacePack => "interface-pack",
            Self::RuntimeScenario => "runtime-scenario",
            Self::VerificationDisposition => "verification-disposition",
            Self::ReviewedKnowledge => "reviewed-knowledge",
            Self::ProjectComposition => "project-composition",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockerProducerEffect {
    Closes,
    Delegated,
    DownstreamOnly,
    Informational,
    Unsupported,
}

impl BlockerProducerEffect {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Closes => "closes",
            Self::Delegated => "delegated",
            Self::DownstreamOnly => "downstream-only",
            Self::Informational => "informational",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockerResolutionRecordKind {
    ReviewedFunctionFact,
    ReviewedKnowledgeAssertion,
    InterfaceAnchorOrSlot,
    VerificationScenario,
    VerificationDisposition,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockerCompletionKind {
    DiagnosticRootAbsent,
    EventRouteBlockerAbsent,
    DelegatedRootsAbsent,
    DownstreamEvidenceAccepted,
    InformationalMarker,
    Unsupported,
}

impl BlockerCompletionKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::DiagnosticRootAbsent => "diagnostic-root-absent",
            Self::EventRouteBlockerAbsent => "event-route-blocker-absent",
            Self::DelegatedRootsAbsent => "delegated-roots-absent",
            Self::DownstreamEvidenceAccepted => "downstream-evidence-accepted",
            Self::InformationalMarker => "informational-marker",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BlockerCompletionPredicate {
    pub kind: BlockerCompletionKind,
    pub producer: String,
    pub root_id: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BlockerResolutionRoute {
    pub owner: BlockerResolutionOwner,
    pub required_model: String,
    pub evidence_required: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_kind: Option<BlockerResolutionRecordKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_action: Option<String>,
    pub producer_effect: BlockerProducerEffect,
    /// Whether accepting the named record or implementing the named owner is
    /// expected to remove the linked-IR producer diagnostic.
    pub closes_producer: bool,
    pub completion_predicate: BlockerCompletionPredicate,
    pub rationale: String,
}

impl BlockerResolutionRoute {
    pub(crate) fn validate(&self, root_id: &str) -> Result<()> {
        self.validate_for_producer(
            root_id,
            "authenticated-linked-ir-review-scopes",
            BlockerCompletionKind::DiagnosticRootAbsent,
        )
    }

    pub(crate) fn validate_event_route(&self, route_id: &str, blocker_kind: &str) -> Result<()> {
        self.validate_for_producer(
            &event_route_blocker_root(route_id, blocker_kind),
            "authenticated-flow-event-route",
            BlockerCompletionKind::EventRouteBlockerAbsent,
        )
    }

    fn validate_for_producer(
        &self,
        root_id: &str,
        producer: &str,
        closing_completion: BlockerCompletionKind,
    ) -> Result<()> {
        if self.required_model.is_empty()
            || self.evidence_required.is_empty()
            || self.evidence_required.iter().any(String::is_empty)
            || self.rationale.is_empty()
        {
            return Err(crate::Error::invalid(
                "blocker resolution route has an empty requirement or rationale",
            ));
        }
        if self.completion_predicate.root_id != root_id
            || self.completion_predicate.producer != producer
        {
            return Err(crate::Error::invalid(
                "blocker resolution route does not name its authenticated producer root",
            ));
        }
        let destination_record = (
            self.destination.is_some(),
            self.record_kind.is_some(),
            self.record_action.is_some(),
        );
        if !matches!(
            destination_record,
            (true, true, true) | (false, false, false)
        ) {
            return Err(crate::Error::invalid(
                "blocker resolution route has a partial writable destination contract",
            ));
        }
        if self.closes_producer != (self.producer_effect == BlockerProducerEffect::Closes) {
            return Err(crate::Error::invalid(
                "blocker resolution route producer effect contradicts closes_producer",
            ));
        }
        let expected_completion = match self.producer_effect {
            BlockerProducerEffect::Closes => closing_completion,
            BlockerProducerEffect::Delegated => BlockerCompletionKind::DelegatedRootsAbsent,
            BlockerProducerEffect::DownstreamOnly => {
                BlockerCompletionKind::DownstreamEvidenceAccepted
            }
            BlockerProducerEffect::Informational => BlockerCompletionKind::InformationalMarker,
            BlockerProducerEffect::Unsupported => BlockerCompletionKind::Unsupported,
        };
        if self.completion_predicate.kind != expected_completion {
            return Err(crate::Error::invalid(
                "blocker resolution route completion predicate contradicts its producer effect",
            ));
        }
        if matches!(
            self.owner,
            BlockerResolutionOwner::GenericBackend
                | BlockerResolutionOwner::AnalysisAddon
                | BlockerResolutionOwner::Unsupported
        ) && self.destination.is_some()
        {
            return Err(crate::Error::invalid(
                "blocker resolution route offers a project file to an owner without a declarative consumer",
            ));
        }
        if self.owner == BlockerResolutionOwner::Unsupported
            && self.producer_effect != BlockerProducerEffect::Unsupported
        {
            return Err(crate::Error::invalid(
                "unsupported blocker owner must fail closed with an unsupported producer effect",
            ));
        }
        Ok(())
    }

    fn code_change(
        root_id: &str,
        owner: BlockerResolutionOwner,
        required_model: &str,
        rationale: &str,
    ) -> Self {
        Self {
            owner,
            required_model: required_model.to_owned(),
            evidence_required: owner_evidence(owner),
            destination: None,
            record_kind: None,
            record_action: None,
            producer_effect: BlockerProducerEffect::Closes,
            closes_producer: true,
            completion_predicate: completion_predicate(
                root_id,
                BlockerCompletionKind::DiagnosticRootAbsent,
            ),
            rationale: rationale.to_owned(),
        }
    }

    fn unsupported(root_id: &str, kind: &str) -> Self {
        Self {
            owner: BlockerResolutionOwner::Unsupported,
            required_model: format!(
                "classify blocker kind {kind:?} in the generic analyzer before proposing a project record"
            ),
            evidence_required: owner_evidence(BlockerResolutionOwner::Unsupported),
            destination: None,
            record_kind: None,
            record_action: None,
            producer_effect: BlockerProducerEffect::Unsupported,
            closes_producer: false,
            completion_predicate: completion_predicate(root_id, BlockerCompletionKind::Unsupported),
            rationale: "no registered Blobray consumer can safely interpret a project-local record for this blocker kind"
                .to_owned(),
        }
    }

    fn non_closing(
        root_id: &str,
        owner: BlockerResolutionOwner,
        producer_effect: BlockerProducerEffect,
        completion_kind: BlockerCompletionKind,
        required_model: &str,
        rationale: &str,
    ) -> Self {
        Self {
            owner,
            required_model: required_model.to_owned(),
            evidence_required: owner_evidence(owner),
            destination: None,
            record_kind: None,
            record_action: None,
            producer_effect,
            closes_producer: false,
            completion_predicate: completion_predicate(root_id, completion_kind),
            rationale: rationale.to_owned(),
        }
    }
}

pub(crate) fn event_route_blocker_root(route_id: &str, blocker_kind: &str) -> String {
    format!("event-route:{route_id}:{blocker_kind}")
}

pub(crate) fn event_route_blocker_resolution_route(
    route_id: &str,
    blocker_kind: &str,
) -> BlockerResolutionRoute {
    if blocker_kind == "analysis-limit" {
        return unsupported_event_route_resolution(
            route_id,
            blocker_kind,
            "preserve the exact adjustable limit and required larger bound before proposing a resolution action",
            "a generic analysis-limit label does not prove which invocation bound can advance the route",
        );
    }
    let (owner, required_model, rationale) = match blocker_kind {
        "event-replay-invalid" | "event-delivery-not-replayed" => (
            BlockerResolutionOwner::RuntimeScenario,
            "obtain a current scenario-owned replay for the exact reviewed event route",
            "only current replay evidence can close this typed delivery blocker",
        ),
        "dispatch-evidence-mismatch" | "delivery-call-mismatch" => (
            BlockerResolutionOwner::ReviewedKnowledge,
            "review the exact route stage against current authenticated linked IR",
            "the reviewed route and current generated evidence must agree before the route can advance",
        ),
        "case-handler-unreviewed" | "terminal-without-handler" => (
            BlockerResolutionOwner::ProjectFunctionFact,
            "publish the exact reviewed event handler or terminal identity consumed by the route",
            "the configured function facts must name the current selector-specific route stage",
        ),
        "unknown-delivery-output-role" | "delivery-not-executable" => (
            BlockerResolutionOwner::AnalysisAddon,
            "model the delivery operation output role and executable write effect",
            "the route needs a reusable typed output contract rather than a message-derived project hint",
        ),
        "case-dispatch-not-executable" => (
            BlockerResolutionOwner::AnalysisAddon,
            "model the reviewed selector-indexed dispatch target in generated control-flow evidence",
            "the generic route can advance only after the indirect case edge is generated",
        ),
        "broker-prior-listener-result-unproven" => (
            BlockerResolutionOwner::AnalysisAddon,
            "model selector-specific callback return flow for every prior broker listener",
            "a typed callback-return model is required to prove that earlier listeners cannot stop delivery",
        ),
        "incomplete-sink" => (
            BlockerResolutionOwner::GenericBackend,
            "complete the reached sink body, call targets, and transitive generated effects",
            "the event route inherits the exact sink completeness produced by linked IR",
        ),
        "event-receive-result-flow-unproven" => (
            BlockerResolutionOwner::GenericBackend,
            "preserve typed receive-call return provenance into the reached event.run argument",
            "the current route report recomputes the exact receive-result relation from generated evidence",
        ),
        "event-receive-run-order-unproven" => (
            BlockerResolutionOwner::GenericBackend,
            "preserve a complete CFG witness that receive must execute before event.run",
            "the current route report recomputes the required ordering from generated CFG evidence",
        ),
        "event-queue-producer-frontier-incomplete" => (
            BlockerResolutionOwner::GenericBackend,
            "complete and persist exact call-result provenance and every guarded returning leaf of the enqueue-side queue producer",
            "the route can compare queue identities only after the generic linked IR publishes a complete typed producer frontier",
        ),
        "event-queue-producer-frontier-no-match" => (
            BlockerResolutionOwner::GenericBackend,
            "recover a typed canonical value relation between the complete producer return frontier and the receive queue",
            "canonical linked-value evidence must establish the join; differing rendered expressions are not promoted into a semantic mismatch claim",
        ),
        "event-queue-producer-precondition-unproven" => (
            BlockerResolutionOwner::GenericBackend,
            "preserve the exact caller-side state or guard that selects the matching queue-producer return leaf",
            "a possible guarded return is not a path claim until typed generated evidence selects that leaf at the enqueue site",
        ),
        "event-queue-producer-lifetime-unproven" => (
            BlockerResolutionOwner::RuntimeScenario,
            "join the mutable queue selection to one initialization and delivery epoch, or replay that exact epoch",
            "a site-local RAM or volatile MMIO observation does not prove that enqueue and receive use the same queue object lifetime",
        ),
        "callback-store-dominance-unproven" => (
            BlockerResolutionOwner::GenericBackend,
            "preserve a complete CFG dominance witness from callback store to subscription",
            "the current route report recomputes callback publication ordering from generated CFG evidence",
        ),
        "broker-subscriber-lifetime-unproven" => (
            BlockerResolutionOwner::GenericBackend,
            "join subscription insertion and removal into the exact publisher epoch",
            "the current route report recomputes broker subscriber lifetime from typed generated evidence",
        ),
        "rust-boundary-unmapped" => (
            BlockerResolutionOwner::VerificationDisposition,
            "bind the reached vendor boundary to current production verification evidence or an exact reviewed disposition",
            "the route remains incomplete until its reached replacement boundary is explicitly accounted for",
        ),
        _ => {
            return unsupported_event_route_resolution(
                route_id,
                blocker_kind,
                &format!(
                    "classify typed event-route blocker kind {blocker_kind:?} before proposing a writable record"
                ),
                "no typed event-route owner mapping exists for this blocker kind",
            );
        }
    };
    BlockerResolutionRoute {
        owner,
        required_model: required_model.to_owned(),
        evidence_required: owner_evidence(owner),
        destination: None,
        record_kind: None,
        record_action: None,
        producer_effect: BlockerProducerEffect::Closes,
        closes_producer: true,
        completion_predicate: BlockerCompletionPredicate {
            kind: BlockerCompletionKind::EventRouteBlockerAbsent,
            producer: "authenticated-flow-event-route".to_owned(),
            root_id: event_route_blocker_root(route_id, blocker_kind),
        },
        rationale: format!(
            "{rationale}; completion is recomputed for route {route_id:?} and requires blocker {blocker_kind:?} to be absent"
        ),
    }
}

fn unsupported_event_route_resolution(
    route_id: &str,
    blocker_kind: &str,
    required_model: &str,
    rationale: &str,
) -> BlockerResolutionRoute {
    BlockerResolutionRoute {
        owner: BlockerResolutionOwner::Unsupported,
        required_model: required_model.to_owned(),
        evidence_required: owner_evidence(BlockerResolutionOwner::Unsupported),
        destination: None,
        record_kind: None,
        record_action: None,
        producer_effect: BlockerProducerEffect::Unsupported,
        closes_producer: false,
        completion_predicate: BlockerCompletionPredicate {
            kind: BlockerCompletionKind::Unsupported,
            producer: "authenticated-flow-event-route".to_owned(),
            root_id: event_route_blocker_root(route_id, blocker_kind),
        },
        rationale: rationale.to_owned(),
    }
}

fn owner_evidence(owner: BlockerResolutionOwner) -> Vec<String> {
    let values: &[&str] = match owner {
        BlockerResolutionOwner::GenericBackend => &[
            "source-qualified linked-IR diagnostic with the exact instruction/site",
            "minimal generic regression fixture that removes the same producer root",
        ],
        BlockerResolutionOwner::AnalysisAddon => &[
            "target- and vendor-revision-bounded behavior evidence",
            "executable provider-model regression for the exact call or memory object",
        ],
        BlockerResolutionOwner::ProjectFunctionFact => &[
            "manually verified sparse function fact",
            "function-pack validation against the current authenticated input",
        ],
        BlockerResolutionOwner::InterfacePack => &[
            "exact generated interface observation for this call site",
            "reviewed ABI/layout evidence naming the matching contract and anchor",
        ],
        BlockerResolutionOwner::RuntimeScenario => &[
            "smallest concrete inputs that reproduce the path",
            "fresh production execution trace under that scenario",
        ],
        BlockerResolutionOwner::VerificationDisposition => &[
            "current production component identity",
            "accepted comparison evidence or an exact reviewed policy exclusion",
        ],
        BlockerResolutionOwner::ReviewedKnowledge => &[
            "exact physical subject and current generated observation",
            "applicability-bounded reviewed assertion",
        ],
        BlockerResolutionOwner::ProjectComposition => &[
            "authenticated source binding and symbol inventory",
            "non-empty linked-IR profile covering the declared public family",
        ],
        BlockerResolutionOwner::Unsupported => &[
            "source-qualified minimal reproducer",
            "typed producer cause classification before selecting a consumer",
        ],
    };
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn completion_predicate(root_id: &str, kind: BlockerCompletionKind) -> BlockerCompletionPredicate {
    BlockerCompletionPredicate {
        kind,
        producer: "authenticated-linked-ir-review-scopes".to_owned(),
        root_id: root_id.to_owned(),
    }
}

fn one_path<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> Option<PathBuf> {
    let mut paths = paths.into_iter();
    let first = paths.next()?.clone();
    paths.next().is_none().then_some(first)
}

struct ProjectRecordSpec<'a> {
    required_model: &'a str,
    destination: Option<&'a Path>,
    record_kind: BlockerResolutionRecordKind,
    record_action: &'a str,
    producer_effect: BlockerProducerEffect,
    rationale: &'a str,
}

fn project_record(
    root_id: &str,
    owner: BlockerResolutionOwner,
    spec: ProjectRecordSpec<'_>,
) -> BlockerResolutionRoute {
    BlockerResolutionRoute {
        owner,
        required_model: spec.required_model.to_owned(),
        evidence_required: owner_evidence(owner),
        destination: spec.destination.map(Path::to_path_buf),
        record_kind: spec.destination.map(|_| spec.record_kind),
        record_action: spec.destination.map(|_| spec.record_action.to_owned()),
        producer_effect: spec.producer_effect,
        closes_producer: spec.producer_effect == BlockerProducerEffect::Closes,
        completion_predicate: completion_predicate(
            root_id,
            match spec.producer_effect {
                BlockerProducerEffect::Closes => BlockerCompletionKind::DiagnosticRootAbsent,
                BlockerProducerEffect::Delegated => BlockerCompletionKind::DelegatedRootsAbsent,
                BlockerProducerEffect::DownstreamOnly => {
                    BlockerCompletionKind::DownstreamEvidenceAccepted
                }
                BlockerProducerEffect::Informational => BlockerCompletionKind::InformationalMarker,
                BlockerProducerEffect::Unsupported => BlockerCompletionKind::Unsupported,
            },
        ),
        rationale: spec.rationale.to_owned(),
    }
}

/// Resolve a producer diagnostic to the component that can actually consume
/// the missing information. Message inspection only refines a registered
/// diagnostic kind; unknown kinds fail closed as `unsupported`.
pub(crate) fn blocker_resolution_route(
    project: &ProjectSpec,
    root_id: &str,
    kind: &str,
    message: &str,
) -> BlockerResolutionRoute {
    const BACKEND_REASON: &str = "the missing capability belongs to reusable analysis logic; no project TOML record currently closes it";
    const ADDON_REASON: &str = "the missing target or platform model belongs to compiled analysis knowledge; no project TOML record currently closes it";

    match kind {
        "analysis-budget" | "max-visited-nodes" | "max-examined-edges" => {
            BlockerResolutionRoute::code_change(
                root_id,
                BlockerResolutionOwner::GenericBackend,
                "make exploration complete under an explicit bound or expose a safe higher bound",
                BACKEND_REASON,
            )
        }
        "decode" => BlockerResolutionRoute::code_change(
            root_id,
            BlockerResolutionOwner::GenericBackend,
            "implement the missing instruction decoder and architecture-neutral semantic effect",
            BACKEND_REASON,
        ),
        "call-shape" => BlockerResolutionRoute::code_change(
            root_id,
            BlockerResolutionOwner::GenericBackend,
            "support the observed call shape without discarding typed call effects",
            BACKEND_REASON,
        ),
        "call-boundary" if message.contains("unmodeled-reviewed-external-call") => {
            BlockerResolutionRoute::code_change(
                root_id,
                BlockerResolutionOwner::AnalysisAddon,
                "add an executable return/output model for the reviewed external call",
                ADDON_REASON,
            )
        }
        "call-boundary" => BlockerResolutionRoute::non_closing(
            root_id,
            BlockerResolutionOwner::GenericBackend,
            BlockerProducerEffect::Delegated,
            BlockerCompletionKind::DelegatedRootsAbsent,
            "inspect and close the named callee cause before reevaluating the caller summary",
            "this wrapper delegates to causal callee diagnostics; it has no independent project record and must not be marked closed on its own",
        ),
        "call-result-model" => BlockerResolutionRoute::code_change(
            root_id,
            BlockerResolutionOwner::AnalysisAddon,
            "add an executable return and output-memory model for the named callee",
            ADDON_REASON,
        ),
        "indirect-control-flow" => BlockerResolutionRoute::code_change(
            root_id,
            BlockerResolutionOwner::InterfacePack,
            "resolve the exact indirect target set from generated interface evidence and a reviewed contract anchor",
            "the reviewed interface pack feeds linked-IR target registration, but this diagnostic alone does not prove a contract/anchor; use a correlated interface-layout finding before editing the pack",
        ),
        "memory-load" | "memory-store" => BlockerResolutionRoute::code_change(
            root_id,
            BlockerResolutionOwner::AnalysisAddon,
            "identify the concrete memory object and add executable address, lifetime, and access semantics",
            "reviewed function types are presentation evidence today; executable memory objects belong to analysis knowledge and cannot be replaced by an unconsumed TOML hint",
        ),
        "unresolved-call" => BlockerResolutionRoute::code_change(
            root_id,
            BlockerResolutionOwner::AnalysisAddon,
            "bind authoritative linked code or implement an explicit external-call model",
            ADDON_REASON,
        ),
        "poll-model" => BlockerResolutionRoute::non_closing(
            root_id,
            BlockerResolutionOwner::AnalysisAddon,
            BlockerProducerEffect::Informational,
            BlockerCompletionKind::InformationalMarker,
            "use a bounded MMIO response sequence only for concrete downstream replay",
            "the producer already recognized a reference-modeled polling loop; a runtime scenario does not close linked IR",
        ),
        "memory-intrinsic" => BlockerResolutionRoute::non_closing(
            root_id,
            BlockerResolutionOwner::AnalysisAddon,
            BlockerProducerEffect::Informational,
            BlockerCompletionKind::InformationalMarker,
            "retain the recognized standard-memory intrinsic marker as informational evidence",
            "this marker is emitted after the intrinsic is modeled and is not a missing project fact",
        ),
        "control-flow" => BlockerResolutionRoute::code_change(
            root_id,
            BlockerResolutionOwner::GenericBackend,
            "recover the unresolved branch input or extend bounded control-flow execution",
            BACKEND_REASON,
        ),
        "aggregate" => BlockerResolutionRoute::non_closing(
            root_id,
            BlockerResolutionOwner::GenericBackend,
            BlockerProducerEffect::Delegated,
            BlockerCompletionKind::DelegatedRootsAbsent,
            "remove the child producer diagnostics represented by this aggregate",
            "an aggregate wrapper has no independent record and disappears only after its causal child diagnostics close",
        ),
        "other" if message.contains("unmodeled-reviewed-external-call") => {
            BlockerResolutionRoute::code_change(
                root_id,
                BlockerResolutionOwner::AnalysisAddon,
                "add an executable model for the named reviewed external operation",
                ADDON_REASON,
            )
        }
        "other" if message.contains("unmapped-register ") => project_record(
            root_id,
            BlockerResolutionOwner::ReviewedKnowledge,
            ProjectRecordSpec {
                required_model: "name and review the exact generated unmapped register observation",
                destination: project.reviewed_knowledge_default.as_deref(),
                record_kind: BlockerResolutionRecordKind::ReviewedKnowledgeAssertion,
                record_action: "add a register-identity assertion only for the exact generated register-model subject",
                producer_effect: BlockerProducerEffect::Closes,
                rationale: "the selected reviewed-knowledge pack is consumed by register-model composition before linked IR is regenerated",
            },
        ),
        "other"
            if message.contains("standard-memory-intrinsic")
                || message.contains("branch-aware memory composition")
                || message.contains("unresolved after argument substitution")
                || message.contains("unsupported execution edge") =>
        {
            BlockerResolutionRoute::code_change(
                root_id,
                BlockerResolutionOwner::GenericBackend,
                "implement the missing memory, composition, or execution semantic named by the diagnostic",
                BACKEND_REASON,
            )
        }
        "other"
            if message.contains("exceeds the limit of")
                && message.contains("distinct effect variants") =>
        {
            BlockerResolutionRoute::code_change(
                root_id,
                BlockerResolutionOwner::GenericBackend,
                "preserve every distinct site effect under an explicit scalable variant bound",
                BACKEND_REASON,
            )
        }
        "other" if message.starts_with("decode-blocker class=") => {
            BlockerResolutionRoute::code_change(
                root_id,
                BlockerResolutionOwner::GenericBackend,
                "classify the exact reachable bytes as code, data, padding, or a supported instruction",
                BACKEND_REASON,
            )
        }
        "other" if message.starts_with("path-local-composition:") => {
            BlockerResolutionRoute::code_change(
                root_id,
                BlockerResolutionOwner::GenericBackend,
                "retain exact call-token, result-object, and memory-write provenance during path-local composition",
                BACKEND_REASON,
            )
        }
        "replacement-uncovered" | "replacement-probe-only" | "replacement-unmapped" => {
            let destination = project.verification.as_ref().and_then(|verification| {
                one_path(
                    verification
                        .suites
                        .iter()
                        .flat_map(|suite| suite.dispositions.iter()),
                )
            });
            project_record(
                root_id,
                BlockerResolutionOwner::VerificationDisposition,
                ProjectRecordSpec {
                    required_model: "bind the production component and obtain accepted current verification evidence or an exact reviewed exclusion",
                    destination: destination.as_deref(),
                    record_kind: BlockerResolutionRecordKind::VerificationDisposition,
                    record_action: "record the evidence-backed production binding or exact policy exclusion, then rerun verification",
                    producer_effect: BlockerProducerEffect::Closes,
                    rationale: "replacement coverage is recomputed from the current verification report and exact policy dispositions",
                },
            )
        }
        "replacement-implemented-unqualified" => {
            let destination = project.verification.as_ref().and_then(|verification| {
                one_path(
                    verification
                        .suites
                        .iter()
                        .flat_map(|suite| suite.profiles.iter()),
                )
            });
            project_record(
                root_id,
                BlockerResolutionOwner::RuntimeScenario,
                ProjectRecordSpec {
                    required_model: "obtain a current production trace and accepted matching verification case",
                    destination: destination.as_deref(),
                    record_kind: BlockerResolutionRecordKind::VerificationScenario,
                    record_action: "add the smallest concrete production scenario that qualifies this replacement",
                    producer_effect: BlockerProducerEffect::Closes,
                    rationale: "the finding disappears only when the fresh verification report is Match and names a production component",
                },
            )
        }
        "replacement-bounded" => BlockerResolutionRoute::non_closing(
            root_id,
            BlockerResolutionOwner::VerificationDisposition,
            BlockerProducerEffect::Informational,
            BlockerCompletionKind::InformationalMarker,
            "retain the finite bounded claim or collect stronger whole-function verification evidence",
            "the bounded result is a terminal finite claim, not an unresolved analyzer model",
        ),
        "replacement-incomplete" | "replacement-mismatch" => BlockerResolutionRoute::non_closing(
            root_id,
            BlockerResolutionOwner::Unsupported,
            BlockerProducerEffect::Unsupported,
            BlockerCompletionKind::Unsupported,
            "inspect the typed verification cause and fix production or its executable model before rerunning comparison",
            "a disposition cannot turn incomplete or mismatching execution into a successful comparison",
        ),
        // These typed kinds are used by focused investigations and future
        // producers. They demonstrate the only situations in which a project
        // destination is offered.
        "function-fact" => project_record(
            root_id,
            BlockerResolutionOwner::ProjectFunctionFact,
            ProjectRecordSpec {
                required_model: "record the manually verified function signature, precondition, path, or logical type",
                destination: project.functions.as_ref().map(|paths| paths.pack.as_path()),
                record_kind: BlockerResolutionRecordKind::ReviewedFunctionFact,
                record_action: "add only the verified sparse function fact",
                producer_effect: BlockerProducerEffect::DownstreamOnly,
                rationale: "the function pack is consumed by focused inspection, but it does not currently remove linked-IR producer diagnostics",
            },
        ),
        "interface-layout" => project_record(
            root_id,
            BlockerResolutionOwner::InterfacePack,
            ProjectRecordSpec {
                required_model: "bind the generated interface observation to a reviewed contract anchor and slot",
                destination: project
                    .interfaces
                    .as_ref()
                    .and_then(|paths| paths.pack.as_deref()),
                record_kind: BlockerResolutionRecordKind::InterfaceAnchorOrSlot,
                record_action: "add the evidence-backed anchor or slot selected by generated interface facts",
                producer_effect: BlockerProducerEffect::Closes,
                rationale: "the reviewed interface pack is a linked-IR input, but only generated matching evidence may justify the record",
            },
        ),
        "runtime-scenario" => {
            let destination = project.verification.as_ref().and_then(|verification| {
                one_path(
                    verification
                        .suites
                        .iter()
                        .flat_map(|suite| suite.profiles.iter()),
                )
            });
            project_record(
                root_id,
                BlockerResolutionOwner::RuntimeScenario,
                ProjectRecordSpec {
                    required_model: "add the smallest concrete scenario that exercises the unresolved runtime state",
                    destination: destination.as_deref(),
                    record_kind: BlockerResolutionRecordKind::VerificationScenario,
                    record_action: "add a bounded scenario to the uniquely selected verification profile",
                    producer_effect: BlockerProducerEffect::DownstreamOnly,
                    rationale: "verification scenarios are consumed by comparison runs, not by linked-IR production",
                },
            )
        }
        "verification-disposition" => {
            let destination = project.verification.as_ref().and_then(|verification| {
                one_path(
                    verification
                        .suites
                        .iter()
                        .flat_map(|suite| suite.dispositions.iter()),
                )
            });
            project_record(
                root_id,
                BlockerResolutionOwner::VerificationDisposition,
                ProjectRecordSpec {
                    required_model: "record the reviewed vendor/Rust semantic disposition",
                    destination: destination.as_deref(),
                    record_kind: BlockerResolutionRecordKind::VerificationDisposition,
                    record_action: "add a reviewed disposition backed by the observed comparison evidence",
                    producer_effect: BlockerProducerEffect::DownstreamOnly,
                    rationale: "a disposition classifies verification evidence and cannot hide an analyzer producer diagnostic",
                },
            )
        }
        _ => BlockerResolutionRoute::unsupported(root_id, kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_spec() -> ProjectSpec {
        ProjectSpec {
            id: "fixture".to_owned(),
            target_spec: "target.toml".into(),
            ecosystem_packs: Vec::new(),
            chip_pack: None,
            analysis_provider: None,
            run_spec: None,
            memory_map: None,
            svd_paths: Vec::new(),
            reviewed_knowledge: Vec::new(),
            reviewed_knowledge_default: None,
            review_context: open_radio_vendor_contracts::ApplicabilityContext::default(),
            symbol_inventory: None,
            navigation_index: None,
            code: None,
            ir_profiles: Vec::new(),
            analysis_symbol_families: Vec::new(),
            registers: None,
            interfaces: None,
            functions: None,
            review: None,
            verification: None,
        }
    }

    #[test]
    fn current_review_blocker_kinds_have_explicit_non_project_owners() {
        let project = project_spec();
        let cases = [
            (
                "analysis-budget",
                "budget",
                BlockerResolutionOwner::GenericBackend,
            ),
            (
                "call-result-model",
                "result",
                BlockerResolutionOwner::AnalysisAddon,
            ),
            (
                "call-shape",
                "shape",
                BlockerResolutionOwner::GenericBackend,
            ),
            (
                "control-flow",
                "branch",
                BlockerResolutionOwner::GenericBackend,
            ),
            (
                "decode",
                "instruction",
                BlockerResolutionOwner::GenericBackend,
            ),
            (
                "indirect-control-flow",
                "jalr",
                BlockerResolutionOwner::InterfacePack,
            ),
            ("memory-load", "load", BlockerResolutionOwner::AnalysisAddon),
            (
                "memory-store",
                "store",
                BlockerResolutionOwner::AnalysisAddon,
            ),
            (
                "unresolved-call",
                "callee",
                BlockerResolutionOwner::AnalysisAddon,
            ),
        ];
        for (kind, message, owner) in cases {
            let route = blocker_resolution_route(&project, "root", kind, message);
            assert_eq!(route.owner, owner, "kind {kind}");
            assert!(route.destination.is_none(), "kind {kind}");
            assert!(route.record_action.is_none(), "kind {kind}");
            assert!(route.closes_producer, "kind {kind}");
        }

        let delegated =
            blocker_resolution_route(&project, "call-root", "call-boundary", "callee-ineligible");
        assert_eq!(delegated.producer_effect, BlockerProducerEffect::Delegated);
        assert!(!delegated.closes_producer);
    }

    #[test]
    fn heterogeneous_other_kind_fails_closed_without_a_known_pattern() {
        let route = blocker_resolution_route(
            &project_spec(),
            "opaque-root",
            "other",
            "producer emitted an unclassified diagnostic",
        );
        assert_eq!(route.owner, BlockerResolutionOwner::Unsupported);
        assert!(!route.closes_producer);
        assert!(route.destination.is_none());
        assert_eq!(route.completion_predicate.root_id, "opaque-root");
    }

    #[test]
    fn external_call_pattern_is_owned_by_the_analysis_addon() {
        let route = blocker_resolution_route(
            &project_spec(),
            "external-root",
            "other",
            "unmodeled-reviewed-external-call at 0x1000",
        );
        assert_eq!(route.owner, BlockerResolutionOwner::AnalysisAddon);
        assert!(route.destination.is_none());
        assert!(route.record_kind.is_none());
    }

    #[test]
    fn unmapped_register_routes_only_to_the_selected_reviewed_knowledge_pack() {
        let mut project = project_spec();
        project.reviewed_knowledge = vec!["facts.toml".into()];
        project.reviewed_knowledge_default = Some("facts.toml".into());
        let route = blocker_resolution_route(
            &project,
            "register-root",
            "other",
            "call-summary-flattening: unmapped-register 0x2010a804",
        );
        assert_eq!(route.owner, BlockerResolutionOwner::ReviewedKnowledge);
        assert_eq!(route.destination.as_deref(), Some(Path::new("facts.toml")));
        assert_eq!(
            route.record_kind,
            Some(BlockerResolutionRecordKind::ReviewedKnowledgeAssertion)
        );
        assert!(route.closes_producer);
        route.validate("register-root").unwrap();
    }

    #[test]
    fn every_current_producer_kind_has_a_valid_explicit_effect() {
        let project = project_spec();
        let cases = [
            ("aggregate", "aggregate", BlockerProducerEffect::Delegated),
            ("analysis-budget", "budget", BlockerProducerEffect::Closes),
            (
                "call-boundary",
                "callee-ineligible",
                BlockerProducerEffect::Delegated,
            ),
            ("call-result-model", "result", BlockerProducerEffect::Closes),
            ("call-shape", "shape", BlockerProducerEffect::Closes),
            ("control-flow", "branch", BlockerProducerEffect::Closes),
            ("decode", "floating-point", BlockerProducerEffect::Closes),
            (
                "indirect-control-flow",
                "jalr",
                BlockerProducerEffect::Closes,
            ),
            ("memory-load", "load", BlockerProducerEffect::Closes),
            ("memory-store", "store", BlockerProducerEffect::Closes),
            ("poll-model", "poll", BlockerProducerEffect::Informational),
            (
                "memory-intrinsic",
                "memcpy",
                BlockerProducerEffect::Informational,
            ),
            ("unresolved-call", "sprintf", BlockerProducerEffect::Closes),
            ("other", "opaque", BlockerProducerEffect::Unsupported),
            (
                "replacement-uncovered",
                "coverage",
                BlockerProducerEffect::Closes,
            ),
            (
                "replacement-probe-only",
                "probe",
                BlockerProducerEffect::Closes,
            ),
            (
                "replacement-unmapped",
                "mapping",
                BlockerProducerEffect::Closes,
            ),
            (
                "replacement-implemented-unqualified",
                "runtime",
                BlockerProducerEffect::Closes,
            ),
            (
                "replacement-incomplete",
                "incomplete",
                BlockerProducerEffect::Unsupported,
            ),
            (
                "replacement-mismatch",
                "mismatch",
                BlockerProducerEffect::Unsupported,
            ),
            (
                "replacement-bounded",
                "bounded",
                BlockerProducerEffect::Informational,
            ),
            ("max-visited-nodes", "limit", BlockerProducerEffect::Closes),
            ("max-examined-edges", "limit", BlockerProducerEffect::Closes),
        ];
        for (index, (kind, message, effect)) in cases.into_iter().enumerate() {
            let root = format!("root-{index}");
            let route = blocker_resolution_route(&project, &root, kind, message);
            assert_eq!(route.producer_effect, effect, "kind {kind}");
            route.validate(&root).unwrap();
        }
    }

    #[test]
    fn route_validation_rejects_forged_project_targets_and_effects() {
        let mut route = blocker_resolution_route(
            &project_spec(),
            "decode-root",
            "decode",
            "unsupported instruction",
        );
        route.destination = Some("reviewed.toml".into());
        route.record_kind = Some(BlockerResolutionRecordKind::ReviewedFunctionFact);
        route.record_action = Some("hide decoder gap".to_owned());
        assert!(route.validate("decode-root").is_err());

        let mut route =
            blocker_resolution_route(&project_spec(), "budget-root", "analysis-budget", "budget");
        route.closes_producer = false;
        assert!(route.validate("budget-root").is_err());
    }

    #[test]
    fn event_route_blockers_use_typed_owners_and_exact_absence_predicates() {
        let queue = event_route_blocker_resolution_route(
            "route-a",
            "event-queue-producer-frontier-incomplete",
        );
        assert_eq!(queue.owner, BlockerResolutionOwner::GenericBackend);
        assert_eq!(
            queue.completion_predicate.kind,
            BlockerCompletionKind::EventRouteBlockerAbsent
        );
        queue
            .validate_event_route("route-a", "event-queue-producer-frontier-incomplete")
            .unwrap();

        let precondition = event_route_blocker_resolution_route(
            "route-a",
            "event-queue-producer-precondition-unproven",
        );
        assert_eq!(precondition.owner, BlockerResolutionOwner::GenericBackend);
        assert_ne!(precondition.required_model, queue.required_model);
        precondition
            .validate_event_route("route-a", "event-queue-producer-precondition-unproven")
            .unwrap();

        let no_match = event_route_blocker_resolution_route(
            "route-a",
            "event-queue-producer-frontier-no-match",
        );
        assert_eq!(no_match.owner, BlockerResolutionOwner::GenericBackend);
        assert_ne!(no_match.required_model, queue.required_model);
        no_match
            .validate_event_route("route-a", "event-queue-producer-frontier-no-match")
            .unwrap();

        let lifetime = event_route_blocker_resolution_route(
            "route-a",
            "event-queue-producer-lifetime-unproven",
        );
        assert_eq!(lifetime.owner, BlockerResolutionOwner::RuntimeScenario);
        assert_ne!(lifetime.required_model, precondition.required_model);
        lifetime
            .validate_event_route("route-a", "event-queue-producer-lifetime-unproven")
            .unwrap();

        let listener = event_route_blocker_resolution_route(
            "route-a",
            "broker-prior-listener-result-unproven",
        );
        assert_eq!(listener.owner, BlockerResolutionOwner::AnalysisAddon);
        assert_ne!(listener.required_model, queue.required_model);
        listener
            .validate_event_route("route-a", "broker-prior-listener-result-unproven")
            .unwrap();

        let unknown = event_route_blocker_resolution_route("route-a", "future-blocker");
        assert_eq!(unknown.owner, BlockerResolutionOwner::Unsupported);
        assert_eq!(unknown.producer_effect, BlockerProducerEffect::Unsupported);
        unknown
            .validate_event_route("route-a", "future-blocker")
            .unwrap();

        let limit = event_route_blocker_resolution_route("route-a", "analysis-limit");
        assert_eq!(limit.owner, BlockerResolutionOwner::Unsupported);
        assert_eq!(limit.producer_effect, BlockerProducerEffect::Unsupported);
        assert!(!limit.closes_producer);
        assert_eq!(
            limit.completion_predicate.kind,
            BlockerCompletionKind::Unsupported
        );
        limit
            .validate_event_route("route-a", "analysis-limit")
            .unwrap();
    }
}
