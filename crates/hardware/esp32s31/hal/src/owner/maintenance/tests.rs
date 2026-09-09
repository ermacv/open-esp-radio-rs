use super::{Error, Observation, check_stopped};

#[derive(Default)]
struct Hardware {
    active: u8,
    walker: bool,
    mac_reads: usize,
    walker_reads: usize,
}

impl Observation for Hardware {
    fn mac_active(&mut self) -> u8 {
        self.mac_reads += 1;
        self.active
    }
    fn rx_walker_enabled(&mut self) -> bool {
        self.walker_reads += 1;
        self.walker
    }
}

#[test]
fn inactive_mac_does_not_authorize_a_live_rx_walker() {
    let mut hardware = Hardware {
        walker: true,
        ..Hardware::default()
    };
    assert_eq!(check_stopped(&mut hardware), Err(Error::RxWalkerEnabled));
    assert!(hardware.walker, "admission must not silently stop DMA");
    assert_eq!((hardware.mac_reads, hardware.walker_reads), (1, 1));
}

#[test]
fn active_mac_is_rejected_without_polling_or_changing_hardware() {
    let mut hardware = Hardware {
        active: 3,
        walker: true,
        ..Hardware::default()
    };
    assert_eq!(
        check_stopped(&mut hardware),
        Err(Error::MacActive { state: 3 })
    );
    assert_eq!(hardware.active, 3);
    assert!(hardware.walker);
    assert_eq!((hardware.mac_reads, hardware.walker_reads), (1, 0));
}

#[test]
fn release_rechecks_hardware_instead_of_reusing_admission_observation() {
    let mut hardware = Hardware::default();
    assert_eq!(check_stopped(&mut hardware), Ok(()));

    // PHY restoration can change baseband state. A successful entry check
    // cannot certify the caller's postcondition after those writes.
    hardware.active = 1;
    assert_eq!(
        check_stopped(&mut hardware),
        Err(Error::MacActive { state: 1 })
    );
    hardware.active = 0;
    hardware.walker = true;
    assert_eq!(check_stopped(&mut hardware), Err(Error::RxWalkerEnabled));
    hardware.walker = false;
    assert_eq!(check_stopped(&mut hardware), Ok(()));
    assert_eq!((hardware.mac_reads, hardware.walker_reads), (4, 3));
}

fn registers() -> super::RadioRuntimeOwner {
    super::RadioRuntimeOwner::from_pac(oer_esp32s31_pac::validation::wifi_radio_registers())
}

fn checkpoint() -> super::MacInterruptCheckpoint {
    use crate::owner::{MacInterruptRegisters, MacPowerInterruptRegisters};
    MacInterruptRegisters {
        inner: oer_esp32s31_pac::validation::mac_interrupt_registers(),
    }
    .checkpoint(MacPowerInterruptRegisters {
        inner: oer_esp32s31_pac::validation::mac_power_interrupt_registers(),
    })
}

#[test]
fn paused_irq_authority_survives_rejection_and_checked_release_without_cold_setup() {
    let rejected = super::admit(registers(), checkpoint(), |_| Err(Error::RxWalkerEnabled))
        .err()
        .expect("live walker must reject maintenance");
    assert_eq!(rejected.error, Error::RxWalkerEnabled);
    let access = super::admit(rejected.registers, rejected.interrupts, |_| Ok(()))
        .ok()
        .expect("quiescent frontier admits the retained owners");
    let (_registers, checkpoint): (_, super::MacInterruptCheckpoint) =
        access.release_with(|_| Ok(())).ok().expect("restored");
    // This ownership-only operation must not clear peripheral state, perform
    // cold activation, or access hardware on host.
    let (mac, power) = checkpoint.into_registers();
    let _same_epoch = mac.checkpoint(power);
}

#[test]
fn restoration_failure_retains_interrupt_authority_until_fault_is_retired() {
    use std::{cell::Cell, rc::Rc};
    struct InterruptOwner(Rc<Cell<usize>>);
    impl super::sealed::InterruptAuthority for InterruptOwner {}
    impl super::InterruptAuthority for InterruptOwner {}
    impl Drop for InterruptOwner {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = Rc::new(Cell::new(0));
    let access = super::admit(registers(), InterruptOwner(drops.clone()), |_| Ok(()))
        .ok()
        .expect("admitted");
    let fault = access
        .release_with(|_| Err(Error::MacActive { state: 1 }))
        .err()
        .expect("restoration failed");
    assert_eq!(fault.error, Error::MacActive { state: 1 });
    assert_eq!(drops.get(), 0, "failed release must retain IRQ authority");
    drop(fault);
    assert_eq!(drops.get(), 1);
}
