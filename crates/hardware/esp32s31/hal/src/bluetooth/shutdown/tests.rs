use super::*;
struct Model {
    pending: bool,
    separated: bool,
    reset_ok: bool,
    compare_enabled: bool,
    counter_running: bool,
    reset_count: usize,
}
impl Control for Model {
    fn time_pending(&self) -> bool {
        self.pending
    }
    fn timer_separated(&self) -> bool {
        self.separated
    }
    fn disable_compare(&mut self) {
        self.compare_enabled = false;
    }
    fn reset_controller(&mut self) {
        assert!(
            !self.compare_enabled,
            "compare must be disabled before reset"
        );
        self.reset_count += 1;
        if self.reset_ok {
            self.counter_running = false;
        }
    }
    fn reset_released(&self) -> bool {
        self.reset_ok
    }
}
#[test]
fn outstanding_time_and_missing_timer_partition_prevent_shutdown_mutation() {
    for (pending, separated, error) in [
        (
            true,
            true,
            BluetoothPhysicalReleaseError::ControllerTimePending,
        ),
        (
            false,
            false,
            BluetoothPhysicalReleaseError::TimerOwnerPresent,
        ),
    ] {
        let mut model = Model {
            pending,
            separated,
            reset_ok: true,
            compare_enabled: true,
            counter_running: true,
            reset_count: 0,
        };
        assert_eq!(prepare(&mut model), Err(error));
        assert!(model.compare_enabled && model.counter_running);
        assert_eq!(model.reset_count, 0);
    }
}
#[test]
fn reset_readback_is_required_before_returning_hardware_ownership() {
    for reset_ok in [false, true] {
        let mut model = Model {
            pending: false,
            separated: true,
            reset_ok,
            compare_enabled: true,
            counter_running: true,
            reset_count: 0,
        };
        assert_eq!(
            prepare(&mut model),
            if reset_ok {
                Ok(())
            } else {
                Err(BluetoothPhysicalReleaseError::ControllerReset)
            }
        );
        assert!(!model.compare_enabled);
        assert_eq!(model.reset_count, 1);
        assert_eq!(model.counter_running, !reset_ok);
    }
}
