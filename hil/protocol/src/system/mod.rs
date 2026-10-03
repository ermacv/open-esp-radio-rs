//! The system module: SoC diagnostics independent of any radio: the
//! watchdogs, stack high-water marks, the timebase probe and the memory
//! benchmark.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

mod payload;
pub use payload::*;
#[cfg(feature = "system")]
mod memory_benchmark;
#[cfg(feature = "system")]
pub use memory_benchmark::*;

crate::messages! {
    endpoint WatchdogTest = "system/watchdog/test" => WatchdogArmed;
    topic WatchdogArmed = "system/watchdog/armed";
    property TimebaseProbe = "system/timebase-probe";
    property IpcCall = "system/ipc-call";
    property MemoryBenchmark = "system/memory-benchmark";
    endpoint InjectHang = "system/hang/inject" => crate::system::HangInjected;
    topic HangInjected = "system/hang/injected";
    endpoint InjectPanic = "system/panic/inject" => crate::system::PanicInjected;
    topic PanicInjected = "system/panic/injected";
    endpoint GetStacks = "system/stacks/get" => crate::system::Stacks;
    topic Stacks = "system/stacks";
    endpoint GetInterruptStacks = "system/interrupt-stacks/get" => crate::system::InterruptStacks;
    topic InterruptStacks = "system/interrupt-stacks";
    endpoint ProbeTimebase = "system/timebase/probe" => crate::system::TimebaseProbed;
    topic TimebaseProbed = "system/timebase/probed";
    endpoint CallAcrossCores = "system/ipc/call" => crate::system::CoresCalled;
    topic CoresCalled = "system/ipc/called";
    #[cfg(feature = "system")]
    endpoint RunMemoryBenchmark = "system/memory-benchmark/run" => crate::system::MemoryBenchmarkCompleted;
    #[cfg(feature = "system")]
    topic MemoryBenchmarkCompleted = "system/memory-benchmark/completed";
}

/// Arm the SoC deadline watchdog and misbehave as `0` says. Tests the SoC
/// deadline service, not RF cessation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WatchdogTest(pub WatchdogTestMode);

/// The watchdog is armed for this mode; a misbehaving mode then ends in the
/// reset the next boot reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WatchdogArmed(pub WatchdogTestMode);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WatchdogTestMode {
    /// Complete normally, then continue serving commands beyond the budget.
    Complete,
    /// Never return from the synchronous task poll.
    BlockedPoll,
    /// Drop an armed future without completing its physical obligation.
    Cancelled,
    /// Keep executor progress but never acknowledge completion.
    LostCompletion,
    /// Attempt restoration after the hardware deadline.
    LateRestoration,
}

/// What a diagnostic hang stalls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub enum HangTarget {
    /// The core 0 protocol executor.
    ProtocolExecutor,
    /// The core 1 network executor.
    NetworkExecutor,
    /// The protocol console's command consumer, while both executors run.
    Console,
}

/// Diagnostic: stall `HangTarget`'s executor with interrupts enabled, so
/// the hang watchdog records a post-mortem and resets the chip.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct InjectHang(pub crate::system::HangTarget);

/// The stall is armed; the chip resets once the watchdog detects it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HangInjected(pub crate::system::HangTarget);

/// Diagnostic: panic in thread context once the acknowledgement is out, so
/// the platform's panic entry records the panic and resets the chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct InjectPanic;

/// The panic follows this acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PanicInjected;

/// Return the boot-lifetime CPU stack high-water marks. This diagnostic
/// query is valid only outside an active traffic session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetStacks;

/// Correlated response to its request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Stacks(pub crate::system::StackUsage);

/// Sample dedicated IRQ stacks on their own harts in thread mode.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetInterruptStacks;

/// `None` denotes no dedicated stack on that hart (shared task stack or inactive hart).
/// A failed measurement must reject/fail rather than return `None`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct InterruptStacks {
    pub cpu0: Option<crate::system::StackWatermark>,
    pub cpu1: Option<crate::system::StackWatermark>,
}

/// Compare alarm deadlines with the monotonic clock before initializing
/// the radio/network runtime.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ProbeTimebase(pub crate::system::TimebaseProbeRequest);

/// Correlated response to its request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TimebaseProbed(pub crate::system::TimebaseProbeEvidence);

/// Post functions through esp-hal's inter-processor call: one from core 0 to
/// itself, one from core 0 to core 1, and from that one, one from core 1 to
/// core 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CallAcrossCores;

/// The core each posted function ran on, by its route; `None` for a function
/// that did not run within the agent's deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CoresCalled {
    pub core0_to_core0: Option<u8>,
    pub core0_to_core1: Option<u8>,
    pub core1_to_core0: Option<u8>,
}

/// `system/memory-benchmark/run`.
#[cfg(feature = "system")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RunMemoryBenchmark(pub crate::system::MemoryBenchmarkRequest);

/// Correlated terminal result for one memory-copy diagnostic case.
#[cfg(feature = "system")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct MemoryBenchmarkCompleted(pub crate::system::MemoryBenchmarkEvidence);
