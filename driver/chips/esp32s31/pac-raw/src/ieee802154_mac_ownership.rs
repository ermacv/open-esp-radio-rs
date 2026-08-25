//! Affine task/interrupt ownership split for the shared IEEE 802.15.4 MAC
//! register block.
//!
//! The hardware places task-side command, policy and DMA registers beside the
//! interrupt event/status registers in one SVD peripheral. A single raw PAC
//! owner therefore cannot model the public driver's simultaneous task and ISR
//! roles. This module consumes that owner, creates exactly two role handles,
//! and reunites them before the complete peripheral can be recovered.
//!
//! The interrupt handle deliberately has no raw-register accessor. Its surface
//! is limited to the source-confirmed ISR snapshot fields and the generated
//! affine W1C acknowledge transaction.

/// Task-side IEEE 802.15.4 MAC register owner.
///
/// The restricted PAC above this raw crate exposes only reviewed command,
/// policy and DMA operations through this handle.
#[must_use = "the task owner must be reunited with its interrupt owner"]
pub struct TaskRegisters {
    registers: crate::Ieee802154Mac,
}

impl TaskRegisters {
    /// Borrow the raw block for the restricted task-side PAC facade.
    ///
    /// This raw-crate bridge is intentionally hidden from generated API docs.
    /// Shipping consumers receive the higher-level task owner, not this type.
    #[doc(hidden)]
    #[inline]
    pub const fn registers(&self) -> &crate::Ieee802154Mac {
        &self.registers
    }

    /// Mutably borrow the raw block for the restricted task-side PAC facade.
    #[doc(hidden)]
    #[inline]
    pub fn task_mac_mut(&mut self) -> &mut crate::Ieee802154Mac {
        &mut self.registers
    }
}

/// Inactive or ISR-owned IEEE 802.15.4 event/status capability.
///
/// There is no conversion to the task owner and no raw register accessor.
#[must_use = "the interrupt owner must be deactivated and reunited"]
pub struct InterruptRegisters {
    registers: crate::Ieee802154Mac,
}

impl InterruptRegisters {
    /// Sample the complete fourteen-bit event field exactly once.
    #[inline]
    pub fn sample_event_status(
        &self,
    ) -> crate::w1c_register_snapshot::Ieee802154EventStatusSnapshot {
        crate::w1c_register_snapshot::sample_ieee802154_event_status(&self.registers)
    }

    /// Acknowledge exactly one previously sampled event field and consume it.
    #[inline]
    pub fn acknowledge_event_status(
        &mut self,
        snapshot: crate::w1c_register_snapshot::Ieee802154EventStatusSnapshot,
    ) {
        crate::w1c_register_snapshot::acknowledge_ieee802154_event_status(
            &mut self.registers,
            snapshot,
        );
    }

    /// Observe the complete RX status word captured for an RX-abort event.
    #[inline]
    pub fn rx_status_bits(&self) -> u32 {
        self.registers.rx_status().read().bits()
    }

    /// Observe the complete TX status word captured for a TX-abort event.
    #[inline]
    pub fn tx_status_bits(&self) -> u32 {
        self.registers.tx_status().read().bits()
    }

    /// Observe the signed energy-detection result captured for ED-DONE.
    #[inline]
    pub fn ed_rss_code(&self) -> i8 {
        self.registers.ed_config().read().ed_rss_code().bits() as i8
    }

    /// Observe the CCA result captured for ED-DONE.
    #[inline]
    pub fn cca_busy(&self) -> bool {
        self.registers.ed_config().read().cca_busy().bit_is_set()
    }
}

/// Split one unique MAC owner into disjoint task and interrupt roles.
///
/// # Safety invariant
///
/// The only duplicated raw handle is retained privately by
/// [`InterruptRegisters`], which exposes no command, policy, DMA, event-enable,
/// or generic register operation. The high-level PAC similarly keeps the task
/// handle behind a task-only surface. Reuniting consumes both handles and drops
/// the duplicate before returning the original owner.
#[inline]
pub fn split(registers: crate::Ieee802154Mac) -> (TaskRegisters, InterruptRegisters) {
    // SAFETY: `registers` was consumed above. The duplicate remains private in
    // the IRQ role, whose safe methods touch only EVENT_STATUS, RX_STATUS,
    // TX_STATUS and ED_CONFIG observations. The task role exposed by the
    // restricted parent PAC does not offer EVENT_STATUS acknowledge or abort
    // status snapshot methods while the roles are separated.
    let interrupt = unsafe { crate::Ieee802154Mac::steal() };
    (
        TaskRegisters { registers },
        InterruptRegisters {
            registers: interrupt,
        },
    )
}

/// Consume both roles and recover the unique complete MAC owner.
#[inline]
pub fn reunite(task: TaskRegisters, interrupt: InterruptRegisters) -> crate::Ieee802154Mac {
    let TaskRegisters { registers } = task;
    let InterruptRegisters {
        registers: _duplicate,
    } = interrupt;
    registers
}
