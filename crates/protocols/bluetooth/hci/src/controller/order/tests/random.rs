//! Entropy service tests through the production affine command routers.

use super::*;
use crate::{LeRandomSource, LeRandomUnavailable};
use bt_hci::cmd::{info::ReadLocalSupportedCmds, le::LeRand};
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering::Relaxed};

#[derive(Default)]
struct Entropy {
    calls: AtomicU8,
    fail: AtomicBool,
}

impl LeRandomSource for Entropy {
    fn random_bytes(&self) -> Result<[u8; 8], LeRandomUnavailable> {
        let sequence = self.calls.fetch_add(1, Relaxed) + 1;
        if self.fail.load(Relaxed) {
            Err(LeRandomUnavailable)
        } else {
            Ok([sequence; 8])
        }
    }
}

fn execute<'a>(
    endpoints: &mut LeControllerHciEndpoints<'a, NoopRawMutex, 1, 1, 80>,
    ready: LeControllerCommandReady<'a, ()>,
    opcode: Opcode,
    parameters: &[u8],
) -> LeControllerResponsePending<'a, ()> {
    block_on(endpoints.host.write(&RawCommand::new(opcode, parameters))).unwrap();
    let command = intake_command(&endpoints.controller, ready, &mut [0; 80]);
    match endpoints.controller.route_idle_classified_command(command) {
        LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) => pending,
        LeControllerIdleClassifiedCommandRoute::ResetBarrier(barrier) => {
            let LeControllerResetCompletion::ResponsePending(pending) = endpoints
                .controller
                .complete_reset_after_quiescence(barrier)
            else {
                panic!("matching Reset")
            };
            pending
        }
        _ => panic!("not a platform/bootstrap command"),
    }
}

fn publish<'a>(
    endpoints: &LeControllerHciEndpoints<'a, NoopRawMutex, 1, 1, 80>,
    pending: LeControllerResponsePending<'a, ()>,
) -> LeControllerCommandReady<'a, ()> {
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("empty output queue")
    };
    ready
}

fn drain(
    endpoints: &LeControllerHciEndpoints<'_, NoopRawMutex, 1, 1, 80>,
    status: Status,
) -> std::vec::Vec<u8> {
    let mut bytes = [0; 80];
    let ControllerToHostPacket::Event(event) = block_on(endpoints.host.read(&mut bytes)).unwrap()
    else {
        panic!("event")
    };
    let complete = CommandComplete::from_hci_bytes_complete(event.data).unwrap();
    let complete: CommandCompleteWithStatus<'_> = complete.try_into().unwrap();
    assert_eq!(complete.status, status);
    complete.return_param_bytes.to_vec()
}

#[test]
fn retired_endpoint_keeps_entropy_binding_for_the_next_host_generation() {
    let entropy = Entropy::default();
    let mut resources = controller_resources();
    let mut endpoints = resources.split();
    endpoints
        .controller
        .install_random_source(&entropy)
        .unwrap();
    let mut ready = claim_initial_ready(&mut endpoints.controller, ());
    for expected in 1..=2 {
        let pending = execute(&mut endpoints, ready, Reset::OPCODE, &[]);
        ready = publish(&endpoints, pending);
        drain(&endpoints, Status::SUCCESS);
        let pending = execute(&mut endpoints, ready, LeRand::OPCODE, &[]);
        ready = publish(&endpoints, pending);
        assert_eq!(drain(&endpoints, Status::SUCCESS), [expected; 8]);
        if expected == 1 {
            let retired = endpoints
                .controller
                .try_retire_transport(ready)
                .unwrap_or_else(|_| panic!("drained command epoch"));
            let (_, proof) = retired.into_parts();
            let host = endpoints
                .controller
                .restart_transport(proof)
                .unwrap_or_else(|_| panic!("exact retirement proof"));
            let old = core::mem::replace(&mut endpoints.host, host);
            assert_eq!(
                block_on(old.write(&Reset::new())),
                Err(HciChannelError::Closed)
            );
            assert!(
                endpoints
                    .controller
                    .install_random_source(&entropy)
                    .is_err()
            );
            ready = claim_initial_ready(&mut endpoints.controller, ());
        }
    }
    assert_eq!(entropy.calls.load(Relaxed), 2);
}

#[test]
fn source_is_explicit_and_not_used_before_reset_or_for_malformed_commands() {
    let entropy = Entropy::default();
    let mut resources = controller_resources();
    let mut endpoints = resources.split();
    let mut ready = claim_initial_ready(&mut endpoints.controller, ());
    let pending = execute(&mut endpoints, ready, LeRand::OPCODE, &[]);
    ready = publish(&endpoints, pending);
    assert_eq!(drain(&endpoints, HciError::UNKNOWN_CMD.to_status()), [0; 8]);
    endpoints
        .controller
        .install_random_source(&entropy)
        .unwrap();
    assert!(
        endpoints
            .controller
            .install_random_source(&entropy)
            .is_err()
    );
    let pending = execute(&mut endpoints, ready, LeRand::OPCODE, &[]);
    ready = publish(&endpoints, pending);
    drain(&endpoints, HciError::CMD_DISALLOWED.to_status());
    let pending = execute(&mut endpoints, ready, Reset::OPCODE, &[]);
    ready = publish(&endpoints, pending);
    drain(&endpoints, Status::SUCCESS);
    let pending = execute(&mut endpoints, ready, LeRand::OPCODE, &[1]);
    let _ready = publish(&endpoints, pending);
    drain(&endpoints, HciError::INVALID_HCI_PARAMETERS.to_status());
    assert_eq!(entropy.calls.load(Relaxed), 0);
}

#[test]
fn random_response_is_sampled_once_and_retained_through_backpressure() {
    let entropy = Entropy::default();
    let mut resources = controller_resources();
    let mut endpoints = resources.split();
    endpoints
        .controller
        .install_random_source(&entropy)
        .unwrap();
    let ready = claim_initial_ready(&mut endpoints.controller, ());
    let pending = execute(&mut endpoints, ready, Reset::OPCODE, &[]);
    let ready = publish(&endpoints, pending); // Leave Reset in the output queue.
    let mut pending = execute(&mut endpoints, ready, LeRand::OPCODE, &[]);
    for _ in 0..3 {
        let LeControllerResponsePublication::Pending(retained) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("full queue")
        };
        pending = retained;
        assert_eq!(entropy.calls.load(Relaxed), 1);
    }
    drain(&endpoints, Status::SUCCESS);
    let ready = publish(&endpoints, pending);
    assert_eq!(drain(&endpoints, Status::SUCCESS), [1; 8]);
    let pending = execute(&mut endpoints, ready, LeRand::OPCODE, &[]);
    let _ready = publish(&endpoints, pending);
    assert_eq!(drain(&endpoints, Status::SUCCESS), [2; 8]);
}

#[test]
fn failure_returns_no_entropy_and_does_not_reset_or_close_hci() {
    let entropy = Entropy::default();
    let mut resources = controller_resources();
    let mut endpoints = resources.split();
    endpoints
        .controller
        .install_random_source(&entropy)
        .unwrap();
    let ready = claim_initial_ready(&mut endpoints.controller, ());
    let pending = execute(&mut endpoints, ready, Reset::OPCODE, &[]);
    let ready = publish(&endpoints, pending);
    drain(&endpoints, Status::SUCCESS);
    entropy.fail.store(true, Relaxed);
    let pending = execute(&mut endpoints, ready, LeRand::OPCODE, &[]);
    let ready = publish(&endpoints, pending);
    assert_eq!(
        drain(&endpoints, HciError::HARDWARE_FAILURE.to_status()),
        [0; 8]
    );
    assert_eq!(
        endpoints.controller.bootstrap_phase(),
        BootstrapPhase::Configuring
    );
    entropy.fail.store(false, Relaxed);
    let pending = execute(&mut endpoints, ready, LeRand::OPCODE, &[]);
    let _ready = publish(&endpoints, pending);
    assert_eq!(drain(&endpoints, Status::SUCCESS), [2; 8]);
}

#[test]
fn mask_tracks_binding_and_binding_cannot_change_after_bootstrap() {
    for enabled in [false, true] {
        let entropy = Entropy::default();
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        if enabled {
            endpoints
                .controller
                .install_random_source(&entropy)
                .unwrap();
        }
        let ready = claim_initial_ready(&mut endpoints.controller, ());
        let pending = execute(&mut endpoints, ready, Reset::OPCODE, &[]);
        let ready = publish(&endpoints, pending);
        drain(&endpoints, Status::SUCCESS);
        assert!(
            endpoints
                .controller
                .install_random_source(&entropy)
                .is_err()
        );
        let pending = execute(&mut endpoints, ready, ReadLocalSupportedCmds::OPCODE, &[]);
        let _ready = publish(&endpoints, pending);
        let mask: [u8; 64] = drain(&endpoints, Status::SUCCESS).try_into().unwrap();
        assert_eq!(
            <&bt_hci::param::CmdMask>::from_hci_bytes_complete(&mask)
                .unwrap()
                .le_rand(),
            enabled
        );
        assert_eq!(entropy.calls.load(Relaxed), 0);
    }
}

#[test]
fn restart_keeps_source_but_old_host_cannot_request_random_values() {
    let entropy = Entropy::default();
    let mut resources = controller_resources();
    let mut endpoints = resources.split();
    endpoints
        .controller
        .install_random_source(&entropy)
        .unwrap();
    let ready = claim_initial_ready(&mut endpoints.controller, ());
    let pending = execute(&mut endpoints, ready, Reset::OPCODE, &[]);
    let ready = publish(&endpoints, pending);
    drain(&endpoints, Status::SUCCESS);
    let retired = endpoints
        .controller
        .try_retire_transport(ready)
        .unwrap_or_else(|_| panic!("drained"));
    let (_, retired) = retired.into_parts();
    let host = endpoints
        .controller
        .restart_transport(retired)
        .unwrap_or_else(|_| panic!("retired"));
    assert_eq!(
        block_on(endpoints.host.write(&LeRand::new())),
        Err(HciChannelError::Closed)
    );
    endpoints.host = host;
    let ready = claim_initial_ready(&mut endpoints.controller, ());
    let pending = execute(&mut endpoints, ready, Reset::OPCODE, &[]);
    let ready = publish(&endpoints, pending);
    drain(&endpoints, Status::SUCCESS);
    let pending = execute(&mut endpoints, ready, LeRand::OPCODE, &[]);
    let _ready = publish(&endpoints, pending);
    assert_eq!(drain(&endpoints, Status::SUCCESS), [1; 8]);
}

#[test]
fn every_active_role_uses_the_same_entropy_service_without_radio_transition() {
    let entropy = Entropy::default();
    let mut resources = controller_resources();
    let mut endpoints = resources.split();
    endpoints
        .controller
        .install_random_source(&entropy)
        .unwrap();
    let ready = claim_initial_ready(&mut endpoints.controller, ());
    let pending = execute(&mut endpoints, ready, Reset::OPCODE, &[]);
    let mut ready = publish(&endpoints, pending);
    drain(&endpoints, Status::SUCCESS);
    for role in 0..4 {
        block_on(endpoints.host.write(&LeRand::new())).unwrap();
        let command = intake_command(&endpoints.controller, ready, &mut [0; 80]);
        let pending = match role {
            0 => {
                let LeControllerClassifiedCommandRoute::ResponsePending(pending) =
                    endpoints.controller.route_classified_command(command)
                else {
                    panic!("DTM platform response")
                };
                pending
            }
            1 => {
                let LeControllerActiveLegacyAdvertisingCommandRoute::ResponsePending(pending) =
                    endpoints
                        .controller
                        .route_active_legacy_advertising_classified_command(command)
                else {
                    panic!("advertising platform response")
                };
                pending
            }
            2 => {
                let LeControllerActiveLegacyScanningCommandRoute::ResponsePending(pending) =
                    endpoints
                        .controller
                        .route_active_legacy_scanning_classified_command(command)
                else {
                    panic!("scanning platform response")
                };
                pending
            }
            _ => {
                let LeControllerActivePeripheralCommandRoute::ResponsePending(pending) = endpoints
                    .controller
                    .route_active_peripheral_classified_command(command, None, true, false, true)
                else {
                    panic!("peripheral platform response")
                };
                pending
            }
        };
        ready = publish(&endpoints, pending);
        assert_eq!(drain(&endpoints, Status::SUCCESS), [role + 1; 8]);
    }
}

#[test]
fn cross_wired_endpoint_cannot_sample_entropy_or_publish_the_response() {
    let entropy = Entropy::default();
    let mut first = controller_resources();
    let mut endpoints = first.split();
    endpoints
        .controller
        .install_random_source(&entropy)
        .unwrap();
    let mut second = controller_resources();
    let mut wrong = second.split();
    wrong.controller.install_random_source(&entropy).unwrap();
    let ready = claim_initial_ready(&mut endpoints.controller, ());
    let pending = execute(&mut endpoints, ready, Reset::OPCODE, &[]);
    let ready = publish(&endpoints, pending);
    drain(&endpoints, Status::SUCCESS);
    block_on(endpoints.host.write(&LeRand::new())).unwrap();
    let command = intake_command(&endpoints.controller, ready, &mut [0; 80]);
    let LeControllerIdleClassifiedCommandRoute::EndpointMismatch(command) =
        wrong.controller.route_idle_classified_command(command)
    else {
        panic!("foreign epoch")
    };
    assert_eq!(entropy.calls.load(Relaxed), 0);
    let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) =
        endpoints.controller.route_idle_classified_command(command)
    else {
        panic!("matching epoch")
    };
    let LeControllerResponsePublication::EndpointMismatch(pending) =
        pending.try_publish(&wrong.controller)
    else {
        panic!("foreign queue")
    };
    assert_eq!(entropy.calls.load(Relaxed), 1);
    let _ready = publish(&endpoints, pending);
    assert_eq!(drain(&endpoints, Status::SUCCESS), [1; 8]);
}
