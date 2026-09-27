//! Independent SoC deadline fault injection. No RF-stop measurement is implied.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

/// Platform reset classification, independent of radio protocol resets.
/// [`BootEvidence::raw_reset_reason`] keeps the chip's own code, so a reason
/// this classification merges or does not name is never lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResetReason {
    Other,
    Software,
    MainWatchdog1,
    /// Power-on, which on some chips also covers a brownout or super
    /// watchdog; a post-mortem that survived tells them apart.
    PowerOn,
    Brownout,
    MainWatchdog0,
    RtcWatchdog,
    SuperWatchdog,
    /// The USB Serial/JTAG controller's JTAG or UART side reset the core.
    UsbSerialJtag,
    /// A JTAG debugger reset the CPU.
    Jtag,
    /// An exception inside the exception handler.
    CpuLockup,
}

/// Observed cause of the boot identified by the enclosing envelope, and what
/// the previous boot left behind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootEvidence {
    pub reset_reason: ResetReason,
    /// The chip's reset reason code; 0 when the target did not report one.
    #[serde(default)]
    pub raw_reset_reason: u8,
    /// The record the previous boot kept in reset-retained memory; `None`
    /// when no valid record survived, as after a power-on.
    #[serde(default)]
    pub post_mortem: Option<PostMortemSummary>,
}

/// The previous boot's post-mortem record, without its checkpoints, which
/// [`crate::Command::GetPostMortemCheckpoints`] pages through.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostMortemSummary {
    /// Boots since the record was created, including the one that wrote it.
    pub boot_count: u32,
    /// Checkpoints held, oldest first, at most [`POST_MORTEM_CHECKPOINTS`].
    pub checkpoints: u8,
    pub fault: Option<Fault>,
}

/// Checkpoints a post-mortem keeps.
pub const POST_MORTEM_CHECKPOINTS: usize = 32;
/// Checkpoints one [`PostMortemCheckpoints`] page carries.
pub const POST_MORTEM_CHECKPOINT_PAGE: usize = 12;
/// Bytes of a checkpoint name.
pub const CHECKPOINT_NAME_BYTES: usize = 16;

/// How the previous boot ended, when it did not end by an ordinary reset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fault {
    Hang(HangFault),
    Panic(PanicFault),
}

/// A watchdog found an executor making no progress and reset the chip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HangFault {
    pub detected_uptime_ms: u32,
    /// One bit per executor that stopped advancing its heartbeat.
    pub stalled_executors: u8,
    /// The context each hart was interrupted in, by hart index.
    pub harts: [HartState; 2],
    /// The stalled hart's preempted instruction at successive checks.
    pub samples: [u32; 16],
}

/// Where a hart was interrupted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HartState {
    /// Whether the hart took the sampling interrupt; a hart that did not
    /// has its interrupts masked, and its other fields are zero.
    pub responded: bool,
    pub mepc: u32,
    pub ra: u32,
    pub sp: u32,
    pub mcause: u32,
    pub mstatus: u32,
}

/// The previous boot panicked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanicFault {
    pub file: heapless::String<48>,
    pub line: u32,
    pub message: heapless::String<96>,
}

/// A named point the previous boot passed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub name: heapless::String<CHECKPOINT_NAME_BYTES>,
    pub arg: u32,
    pub uptime_ms: u32,
    pub hart: u8,
}

/// Checkpoints `first..` of the previous boot's post-mortem, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostMortemCheckpoints {
    pub first: u8,
    pub checkpoints: heapless::Vec<Checkpoint, POST_MORTEM_CHECKPOINT_PAGE>,
}
