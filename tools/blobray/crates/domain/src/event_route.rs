//! Conditional asynchronous routes over exact retained physical observations.
use crate::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSite {
    pub analysis: FunctionAnalysisId,
    pub record: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteArgument {
    pub call: FlowHop,
    /// Physical RV32 integer ABI word, independent of logical argument naming.
    pub word: u8,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteCase {
    pub condition: SavedSite,
    pub taken: bool,
    pub handler: FlowHop,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectorDeliveryRoute {
    pub dispatch: RouteArgument,
    pub selector: u32,
    /// Receive call argument pointing at the output object.
    pub delivery: RouteArgument,
    pub selector_load: SavedSite,
    pub selector_offset: i32,
    pub selector_width: u8,
    pub case: RouteCase,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventDispatch {
    pub call: FlowHop,
    pub object_word: u8,
    pub queue_word: u8,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackRegistration {
    pub call: FlowHop,
    pub object_word: u8,
    pub callback_word: u8,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EventReceiveOutput {
    /// Low return register from the exact receive call.
    Return,
    /// A pointer-sized load from the receive output argument after the call.
    Argument { word: u8, load: SavedSite },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventReceive {
    pub call: FlowHop,
    pub queue_word: u8,
    pub output: EventReceiveOutput,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticCallbackRoute {
    pub dispatches: Vec<EventDispatch>,
    pub registration: CallbackRegistration,
    pub receive: EventReceive,
    pub invoke: RouteArgument,
    pub callback: FunctionAnalysisId,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerDomain {
    pub attach: FlowHop,
    pub object_word: u8,
    pub selector_word: u8,
    pub selector: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerRoute {
    pub publish: FlowHop,
    pub object_word: u8,
    pub selector_word: u8,
    pub selector: u32,
    pub payload_word: u8,
    pub domain: BrokerDomain,
    pub subscribe: FlowHop,
    pub domain_word: u8,
    pub subscriber_word: u8,
    pub callback_store: SavedSite,
    pub callback_offset: i32,
    pub callback: FunctionAnalysisId,
    pub callback_selector_word: u8,
    pub case: RouteCase,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "route",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum EventRouteMechanism {
    SelectorDelivery(Box<SelectorDeliveryRoute>),
    StaticCallback(Box<StaticCallbackRoute>),
    BrokerSubscription(Box<BrokerRoute>),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedEventRoute {
    pub mechanism: SubjectId,
    pub execution_context: String,
    pub applicability: String,
    pub route: EventRouteMechanism,
    /// Optional contiguous path ending at the dispatch caller.
    pub upstream: Vec<FlowHop>,
    /// Optional contiguous path starting at the selected handler/callback.
    pub terminal: Vec<FlowHop>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventRouteQuery {
    pub revision: RevisionId,
    pub route: ReviewedEventRoute,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteCheckStatus {
    Established,
    Unresolved,
    Mismatch,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteCondition {
    MechanismSemantics,
    ObjectLifetime,
    RegistrationBeforeDispatch,
    DeliveryOrder,
    ExecutionContext,
    RuntimeGuard,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EventRouteRecord {
    Evidence {
        site: SavedSite,
        fact: Box<FunctionRecord>,
    },
    Check {
        name: String,
        status: RouteCheckStatus,
        evidence: Vec<SavedSite>,
    },
    /// Explicit temporal/runtime obligation; static acceptance never discharges it.
    Condition { condition: RouteCondition },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventRouteSummary {
    pub schema: u32,
    pub analyses_read: u64,
    pub request: EventRouteQuery,
    pub established: u64,
    pub unresolved: u64,
    pub mismatched: u64,
    pub conditions: u64,
}

impl ReviewedEventRoute {
    pub fn root(&self) -> &FunctionAnalysisId {
        match &self.route {
            EventRouteMechanism::SelectorDelivery(r) => &r.dispatch.call.caller,
            EventRouteMechanism::StaticCallback(r) => r
                .dispatches
                .first()
                .map_or(&r.registration.call.caller, |d| &d.call.caller),
            EventRouteMechanism::BrokerSubscription(r) => &r.publish.caller,
        }
    }
    pub fn endpoint(&self) -> &FunctionAnalysisId {
        match &self.route {
            EventRouteMechanism::SelectorDelivery(r) => &r.case.handler.callee,
            EventRouteMechanism::StaticCallback(r) => &r.callback,
            EventRouteMechanism::BrokerSubscription(r) => &r.case.handler.callee,
        }
    }
    pub fn visit_calls(&self, mut visit: impl FnMut(&FlowHop)) {
        for call in self.upstream.iter().chain(&self.terminal) {
            visit(call);
        }
        match &self.route {
            EventRouteMechanism::SelectorDelivery(r) => {
                visit(&r.dispatch.call);
                visit(&r.delivery.call);
                visit(&r.case.handler);
            }
            EventRouteMechanism::StaticCallback(r) => {
                for d in &r.dispatches {
                    visit(&d.call);
                }
                visit(&r.registration.call);
                visit(&r.receive.call);
                visit(&r.invoke.call);
            }
            EventRouteMechanism::BrokerSubscription(r) => {
                visit(&r.publish);
                visit(&r.domain.attach);
                visit(&r.subscribe);
                visit(&r.case.handler);
            }
        }
    }
    pub fn allocated_bytes(&self) -> u64 {
        let mut bytes = self.mechanism.allocated_bytes()
            + self.execution_context.capacity() as u64
            + self.applicability.capacity() as u64
            + ((self.upstream.capacity() + self.terminal.capacity())
                * std::mem::size_of::<FlowHop>()) as u64;
        self.visit_calls(|call| {
            bytes += call.caller.allocated_bytes() + call.callee.allocated_bytes()
        });
        bytes
            + match &self.route {
                EventRouteMechanism::SelectorDelivery(r) => {
                    std::mem::size_of::<SelectorDeliveryRoute>() as u64
                        + r.selector_load.analysis.allocated_bytes()
                        + r.case.condition.analysis.allocated_bytes()
                }
                EventRouteMechanism::StaticCallback(r) => {
                    std::mem::size_of::<StaticCallbackRoute>() as u64
                        + (r.dispatches.capacity() * std::mem::size_of::<EventDispatch>()) as u64
                        + r.callback.allocated_bytes()
                        + match &r.receive.output {
                            EventReceiveOutput::Return => 0,
                            EventReceiveOutput::Argument { load, .. } => {
                                load.analysis.allocated_bytes()
                            }
                        }
                }
                EventRouteMechanism::BrokerSubscription(r) => {
                    std::mem::size_of::<BrokerRoute>() as u64
                        + r.callback_store.analysis.allocated_bytes()
                        + r.callback.allocated_bytes()
                        + r.case.condition.analysis.allocated_bytes()
                }
            }
    }
}
