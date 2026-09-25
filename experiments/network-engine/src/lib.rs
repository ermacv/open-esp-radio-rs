#![no_std]
#![forbid(unsafe_code)]

//! Hardware-first synchronous network engine for radio datapath research.
//!
//! This is deliberately not an Embassy driver and not a Xarxa fork. It keeps
//! canonical transport work in bounded general-memory storage, exposes
//! durable radio-keyed demand and constructs selected frames directly into a
//! caller-reserved physical batch. The same engine can therefore be called
//! directly by a fused owner or transported across cores by a separate batch
//! SPSC adapter without changing protocol semantics.

mod address;
mod checksum;
mod egress;
mod engine;
mod payload;
mod physical;
mod selected;
mod work;

pub use address::{Ipv4Address, MacAddress, ResolvedIpv4Route, UdpEndpoint};
pub use egress::{
    AdmissionClass, BatchWriteError, DeferredTxWork, EgressDemand, EgressFlowKey, EgressSelection,
    EgressWorkProvider, EnqueueError, FillFailure, FillOutcome, FillStopReason, FixedEgressQueue,
    RadioEgressKey, RadioPeer, ReservedTxBatch, TrafficIdentifier, TrafficIdentifierError,
};
pub use engine::{
    EngineCounters, IngressDisposition, IngressReport, RadioRouteClassifier, ResearchNetworkConfig,
    ResearchNetworkEngine, TxEnqueueError, TxEnqueueFailure, UdpDatagram,
};
pub use payload::InlinePayload;
pub use physical::{
    PinnedBatchAllocator, PinnedBatchResources, PinnedResearchTxFrame, PinnedReservedTxBatch,
};
pub use selected::{SelectedTxReport, SelectedTxSource};
pub use work::FrameWriteError;
