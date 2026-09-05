use super::{Ieee802154TimingTransitionPort, execute_timing_transition, generated};
use std::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    BasebandV2BodyWithoutFence(u8),
    SharedDelay50,
    RxOnDelay50,
    DeviceFence,
}

struct TracePort {
    steps: Vec<Step>,
}

impl Ieee802154TimingTransitionPort for TracePort {
    fn initialize_baseband_v2_arg_one_body_without_fence(&mut self, gain_parameter: u8) {
        self.steps
            .push(Step::BasebandV2BodyWithoutFence(gain_parameter));
    }

    fn override_shared_tx_on_delay(&mut self, value: generated::Ieee802154SharedTxOnDelayOverride) {
        match value {
            generated::Ieee802154SharedTxOnDelayOverride::Delay50 => {
                self.steps.push(Step::SharedDelay50);
            }
        }
    }

    fn set_rx_on_delay(&mut self, value: generated::Ieee802154RxOnDelay) {
        match value {
            generated::Ieee802154RxOnDelay::Delay50 => {
                self.steps.push(Step::RxOnDelay50);
            }
        }
    }

    fn order_device_accesses(&mut self) {
        self.steps.push(Step::DeviceFence);
    }
}

#[test]
fn transition_has_one_closed_semantic_order() {
    let mut port = TracePort { steps: Vec::new() };

    execute_timing_transition(&mut port, 0x6d);

    assert_eq!(
        port.steps,
        [
            Step::BasebandV2BodyWithoutFence(0x6d),
            Step::SharedDelay50,
            Step::RxOnDelay50,
            Step::DeviceFence,
        ]
    );
}
