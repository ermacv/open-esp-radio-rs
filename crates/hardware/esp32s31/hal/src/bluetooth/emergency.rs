//! Bounded emergency quarantine, without shared reset or clock operations.
//!
//! A stopped scheduler is an observation, not DMA reclamation permission.
//! Both outcomes retain all controller-visible allocations until chip reset.

use core::sync::atomic::{AtomicUsize, Ordering};

const CLOSED: usize = 1 << (usize::BITS - 1);

/// Installation-local admission shared by the ISR and task-side operations.
///
/// Admission and its count occupy one atomic word. Closing the word therefore
/// cannot miss an operation that passed admission. A rejected entry touches
/// no installation state. There is no global critical section or lock held
/// while the admitted operation runs. This gate never reopens after closure.
pub struct EmergencyGate {
    entries: AtomicUsize,
    runner: atomic_waker::AtomicWaker,
}

impl Default for EmergencyGate {
    fn default() -> Self {
        Self::new()
    }
}

impl EmergencyGate {
    /// Construct an open gate before publishing the installation.
    pub const fn new() -> Self {
        Self {
            entries: AtomicUsize::new(0),
            runner: atomic_waker::AtomicWaker::new(),
        }
    }

    /// Enter before reaching registers, retained owners or wake publications.
    ///
    /// The successful atomic update joins the in-flight set before admission
    /// is observed by the caller. Even forgotten entries fail closed: the
    /// count saturates rather than wrapping into permission to stop.
    pub fn enter(&self) -> Result<EmergencyEntry<'_>, EmergencyClosed> {
        self.entries
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |word| {
                (word < CLOSED - 1).then(|| word + 1)
            })
            .map(|_| EmergencyEntry { gate: self })
            .map_err(|_| EmergencyClosed)
    }

    /// Whether emergency quarantine has permanently closed admission.
    pub fn is_closed(&self) -> bool {
        self.entries.load(Ordering::Acquire) & CLOSED != 0
    }

    /// Close admission before masking the installation's CPU routes.
    pub fn close(&self) -> EmergencyClosedGate<'_> {
        self.entries.fetch_or(CLOSED, Ordering::AcqRel);
        self.runner.wake();
        EmergencyClosedGate { gate: self }
    }

    /// Poll the installation's single platform runner under counted admission.
    /// The runner is woken when quarantine closes admission, including while
    /// its modem-timer future waits. No entry remains held across a pending poll.
    pub fn poll_runner<F: core::future::Future>(
        &self,
        future: core::pin::Pin<&mut F>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Result<F::Output, EmergencyClosed>> {
        self.runner.register(cx.waker());
        match self.enter() {
            Ok(_entry) => future.poll(cx).map(Ok),
            Err(closed) => core::task::Poll::Ready(Err(closed)),
        }
    }
}

/// A rejected operation must retain its owners and report poisoning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmergencyClosed;

/// One admitted operation, including its final ISR publications.
#[must_use = "retain the entry through the complete register/state operation"]
pub struct EmergencyEntry<'a> {
    gate: &'a EmergencyGate,
}

impl Drop for EmergencyEntry<'_> {
    fn drop(&mut self) {
        self.gate.entries.fetch_sub(1, Ordering::Release);
    }
}

/// Closed admission; CPU routes must be masked before waiting for entrants.
#[must_use = "mask routes, then wait for the admitted operations to leave"]
pub struct EmergencyClosedGate<'a> {
    gate: &'a EmergencyGate,
}

impl<'a> EmergencyClosedGate<'a> {
    /// Wait a finite number of CPU observations, without holding a lock.
    ///
    /// An exhausted budget gives no authority to touch shared ISR registers.
    /// The open implementation chooses the caller's budget; it is not a
    /// vendor guarantee about ISR latency.
    pub fn drain(self, observations: u32) -> Result<EmergencyDrained<'a>, EmergencyUnconfirmed> {
        for _ in 0..observations {
            if self.gate.entries.load(Ordering::Acquire) == CLOSED {
                return Ok(EmergencyDrained { gate: self.gate });
            }
        }
        Err(EmergencyUnconfirmed::EnteredOperation)
    }
}

/// No previously admitted operation remains and future admission is closed.
/// This proves CPU access exclusion, not device/DMA completion.
#[must_use = "retain CPU exclusion through the emergency register transaction"]
pub struct EmergencyDrained<'a> {
    gate: &'a EmergencyGate,
}

impl EmergencyDrained<'_> {
    /// Check that a storage owner belongs to this exact admission gate.
    pub fn belongs_to(&self, gate: &EmergencyGate) -> bool {
        core::ptr::eq(self.gate, gate)
    }
}

/// Why bounded emergency stopping could not confirm scheduler retirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmergencyUnconfirmed {
    /// An admitted ISR or task operation outlasted the CPU observation budget.
    EnteredOperation,
    /// The MAC diagnostic reads never agreed within one sample's budget.
    Diagnostic,
    /// Command-zero/command-one readiness did not arrive within its budget.
    CommandDrain,
    /// The lifecycle request did not reach BUSY clear within its budget.
    SchedulerBusy,
}

/// Observation of an emergency stop; neither variant authorizes DMA reuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "retain the quarantined installation for either outcome"]
pub enum EmergencyOutcome {
    /// The source-ordered scheduler stop reached a stable BUSY-clear sample.
    Stopped,
    /// Admission is closed, routes are masked and backing stays quarantined.
    Unconfirmed(EmergencyUnconfirmed),
}

/// Vendor assertion threshold before the lifecycle request.
///
/// SOURCE(esp32s31): libbtdm_common[18.o]::r_sym_bt_B8fDByebTHuwRe8FZuRt.
/// The counter increments before comparison with 9999; assertion first runs
/// on failed attempt 10,000. The vendor then loops. We terminate unconfirmed.
pub const EMERGENCY_COMMAND_ATTEMPTS: u32 = 10_000;

/// Vendor counter threshold after the lifecycle request.
///
/// SOURCE(esp32s31): libbtdm_common[18.o]::r_sym_bt_74l62ZLsZuXg67pPHSd7.
/// The counter comparison is against 0x10000. The source takes another BUSY
/// sample before its assertion; our bound stops after exactly 65,536 failed
/// samples, without running that extra sample or the continuing assert loop.
pub const EMERGENCY_BUSY_ATTEMPTS: u32 = 65_536;

/// Restricted emergency register owner, independent of the ordinary driver.
#[must_use = "keep the emergency authority outside the ordinary driver future"]
pub struct EmergencyRegisters {
    registers: oer_esp32s31_pac::BluetoothEmergencyRegisters,
}

impl super::TaskOwner {
    /// Separate the emergency register view from ordinary execution.
    ///
    /// # Safety
    ///
    /// The caller must retain this task behind one installation admission gate
    /// and expose it only through counted finite borrows. Exactly one emergency
    /// view may exist; it must be consumed before extracting the ordinary task.
    /// Emergency access requires closing and draining the same gate. Closure
    /// permanently quarantines the task and its durable hardware-visible memory.
    #[allow(
        unsafe_code,
        reason = "the driver wraps the consumed task in counted admission"
    )]
    pub unsafe fn emergency_registers(&mut self) -> EmergencyRegisters {
        // SAFETY: this function forwards the caller's same exclusive-admission
        // and paired-owner obligations to the restricted PAC views.
        let registers = unsafe { self.registers.emergency_registers() };
        EmergencyRegisters { registers }
    }
}

impl EmergencyRegisters {
    /// Stop within the source-derived budgets after CPU access was excluded.
    /// This operation touches only BLE registers and never resets a module.
    pub fn stop(
        &mut self,
        _drained: &EmergencyDrained<'_>,
        diagnostic: super::BluetoothDiagnosticReadBudget,
    ) -> EmergencyOutcome {
        struct Hardware<'a> {
            registers: &'a mut oer_esp32s31_pac::BluetoothEmergencyRegisters,
            diagnostic: super::BluetoothDiagnosticReadBudget,
        }
        impl StopControl for Hardware<'_> {
            fn busy(&mut self) -> Result<bool, EmergencyUnconfirmed> {
                super::diagnostic::settle(self.diagnostic, || self.registers.scheduler_busy())
                    .map_err(|_| EmergencyUnconfirmed::Diagnostic)
            }
            fn preamble(&mut self) {
                self.registers.preamble();
            }
            fn commands_ready(&mut self) -> bool {
                self.registers.commands_ready()
            }
            fn request(&mut self) {
                self.registers.request();
            }
            fn fence(&mut self) {
                self.registers.fence();
            }
        }
        stop(
            &mut Hardware {
                registers: &mut self.registers,
                diagnostic,
            },
            EMERGENCY_COMMAND_ATTEMPTS,
            EMERGENCY_BUSY_ATTEMPTS,
        )
    }
}

pub(super) trait StopControl {
    fn busy(&mut self) -> Result<bool, EmergencyUnconfirmed>;
    fn preamble(&mut self);
    fn commands_ready(&mut self) -> bool;
    fn request(&mut self);
    fn fence(&mut self);
}

pub(super) fn stop(control: &mut impl StopControl, commands: u32, busy: u32) -> EmergencyOutcome {
    fn run(
        control: &mut impl StopControl,
        commands: u32,
        busy: u32,
    ) -> Result<(), EmergencyUnconfirmed> {
        if !control.busy()? {
            control.fence();
            return Ok(());
        }
        control.preamble();
        let mut ready = false;
        for _ in 0..commands {
            if !control.busy()? || control.commands_ready() {
                ready = true;
                break;
            }
        }
        if !ready {
            return Err(EmergencyUnconfirmed::CommandDrain);
        }
        control.request();
        for _ in 0..busy {
            if !control.busy()? {
                control.fence();
                return Ok(());
            }
        }
        Err(EmergencyUnconfirmed::SchedulerBusy)
    }
    match run(control, commands, busy) {
        Ok(()) => EmergencyOutcome::Stopped,
        Err(cause) => EmergencyOutcome::Unconfirmed(cause),
    }
}

#[cfg(test)]
mod tests;

/// Isolated comparison image: run the shipping transaction with no live ISR
/// or ordinary driver, keeping every original owner through the transaction.
#[cfg(feature = "validation-probes")]
#[allow(
    unsafe_code,
    reason = "isolated validation owners have no competing access"
)]
pub(super) fn for_validation(diagnostic: super::BluetoothDiagnosticReadBudget) -> EmergencyOutcome {
    let (_shared, partitions) = crate::root::RadioHardware::for_validation().into_concurrent(());
    let (mut task, _inactive) =
        super::ClockedOwner::for_validation(partitions.bluetooth).separate_interrupt_owner();
    let gate = EmergencyGate {
        entries: AtomicUsize::new(CLOSED),
        runner: atomic_waker::AtomicWaker::new(),
    };
    let drained = EmergencyDrained { gate: &gate };
    // SAFETY: this image retains the original task without exposing it, has
    // no ISR/task entrants, and never uses the original owner again.
    let mut emergency = unsafe { task.emergency_registers() };
    emergency.stop(&drained, diagnostic)
}
