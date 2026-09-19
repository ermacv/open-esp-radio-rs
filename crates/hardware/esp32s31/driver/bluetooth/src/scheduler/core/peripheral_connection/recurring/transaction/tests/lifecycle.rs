//! Deterministic composition of the production completion spine, LL proposal,
//! ACL owner, HCI credit transport and deadline admission. Hardware observations
//! are scripted; the RISC-V active runner and physical DMA remain HIL scope.

use super::*;
use crate::{
    SchedulerInstant,
    le::peripheral::{
        active_acl::{ControllerAclPublication, PeripheralConnectionAcl},
        connection::PeripheralConnectionRecurrence,
        deadlines::{Deadlines, Decision, Expired},
        maintenance::{PeripheralMaintenanceBlocked, PeripheralMaintenanceBudget},
        procedure::PeripheralProcedureDeadline,
        supervision::PeripheralSupervisionDeadline,
    },
    scheduler::completion::{SingleItemCompletion, SingleItemCompletionStep},
};
use core::num::NonZeroU32;
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_bluetooth_hci::{
    BluetoothPublicDeviceAddress, LeControllerActivePeripheralIntake as Intake,
    LeControllerBootstrapConfig, LeControllerCommandReadyClaim, LeControllerHciResources,
    LeHostAclPacket,
    bt_hci::{
        data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
        param::{ConnHandle, ConnHandleCompletedPackets},
        transport::Transport,
    },
};
use oer_bluetooth_ll::{
    connection::{LeChannelSelectionAlgorithm, maintenance::SkipBlocked},
    control::{LePeripheralControl, LePeripheralReceive},
};

#[path = "lifecycle/irq.rs"]
mod irq;

fn at(value: u32) -> SchedulerInstant {
    SchedulerInstant::from_image(value)
}

fn flight() -> SingleItemCompletion<irq::Role> {
    let mut acl = PeripheralConnectionAcl::new();
    let packet = LeHostAclPacket::copy_from(
        AclPacket::new(
            ConnHandle::new(1),
            AclPacketBoundary::FirstNonFlushable,
            AclBroadcastFlag::PointToPoint,
            &[0x5a; 47],
        ),
        Some(ConnHandle::new(1)),
        251,
    )
    .unwrap();
    acl.accept_host_packet(Ok(packet));
    acl.fragment_enqueued(23);
    SingleItemCompletion::new(irq::Owners {
        event: LePeripheralConnection::from_request(
            request(6, 4),
            LeChannelSelectionAlgorithm::AlgorithmTwo,
        )
        .prepare_event()
        .into_submitted(),
        acl,
        control: LePeripheralControl::new(),
    })
}

fn completed_after_delayed_irq() -> irq::Completed {
    let mut hw = irq::Hardware::default();
    let mut completion = flight();
    for _ in 0..3 {
        completion = irq::wait(completion, &mut hw);
    }
    // A wake without a finished role list must not release the RX reservation.
    hw.wake = true;
    completion = irq::advance(completion, &mut hw);
    completion = irq::wait(completion, &mut hw);
    completion = irq::wait(completion, &mut hw);
    assert_eq!(hw.completions, 0);
    hw.wake = true;
    hw.finished = true;
    for _ in 0..4 {
        completion = irq::advance(completion, &mut hw);
    }
    for _ in 0..3 {
        completion = irq::wait(completion, &mut hw);
    }
    assert_eq!((hw.completions, hw.unlinks), (1, 1));
    hw.post_unlink_ready = true;
    let SingleItemCompletionStep::RemovalReady(owner) = completion.step(&mut hw) else {
        panic!("only post-unlink readiness releases the complete owner")
    };
    assert!(owner.acl.controller_event_is_reserved());
    assert!(!owner.acl.can_accept_host_packet());
    owner
}

type Hci = LeControllerHciResources<NoopRawMutex, 2, 2, 80>;
fn hci() -> Hci {
    Hci::new(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
            27,
            1,
        )
        .unwrap(),
    )
    .unwrap()
}

fn receive_data(owner: &mut irq::Completed, pdu: &[u8]) {
    let LePeripheralReceive::Data(fragment) = owner.control.receive(pdu, None).unwrap() else {
        panic!("expected a data fragment")
    };
    owner.acl.accept_controller_fragment(fragment);
}

#[test]
fn saturated_acl_instant_and_maintenance_preserve_the_same_timeline() {
    for anchor in [1_000, u32::MAX - 10_000] {
        let mut owner = completed_after_delayed_irq();
        receive_data(&mut owner, &[2, 1, 0xa1]);
        receive_data(&mut owner, &[1, 1, 0xa2]);
        owner.acl.complete_controller_event();
        assert!(!owner.acl.reserve_controller_event());
        // Control parsing remains possible with the entire software RX queue full.
        let indication = [7, 8, 1, 3, 0, 0, 0, 0, 6, 0];
        assert!(!LePeripheralControl::receive_requires_acl_slot(&indication).unwrap());
        let LePeripheralReceive::ChannelMapUpdate(update) =
            owner.control.receive(&indication, None).unwrap()
        else {
            panic!("queued data must not obscure the Instant")
        };
        owner
            .event
            .schedule_channel_map_update(update.channel_map(), update.instant())
            .unwrap();
        owner.event.observe_valid_packet_header(indication[0]);
        let delta = LePeripheralConnectionEventDelta::from_skipped(2).unwrap();
        assert_eq!(
            owner.event.maintenance_skip_eligible(delta),
            Err(SkipBlocked::InstantAcknowledgement)
        );
        owner.event.observe_valid_packet_header(0x0d);
        assert_eq!(owner.event.maintenance_skip_eligible(delta), Ok(()));
        assert_eq!(
            owner.event.maintenance_skip_eligible(
                LePeripheralConnectionEventDelta::from_skipped(5).unwrap()
            ),
            Err(SkipBlocked::InstantProcedure)
        );

        let mut resources = hci();
        let mut endpoints = resources.split();
        let mut scratch = [0; 80];
        assert!(matches!(
            owner
                .acl
                .try_publish_controller_packet(&endpoints.controller, 27, true, Some(1)),
            ControllerAclPublication::Published
        ));
        let _: oer_bluetooth_hci::bt_hci::ControllerToHostPacket<'_> =
            block_on(endpoints.host.read(&mut scratch)).unwrap();
        assert!(matches!(
            owner
                .acl
                .try_publish_controller_packet(&endpoints.controller, 27, true, Some(1)),
            ControllerAclPublication::FlowControlled
        ));
        assert!(!owner.acl.reserve_controller_event());
        let LeControllerCommandReadyClaim::Ready(ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("command owner")
        };
        block_on(
            endpoints
                .host
                .acl_credit_sender()
                .return_completed_packets(&[ConnHandleCompletedPackets::new(
                    ConnHandle::new(1),
                    1,
                )]),
        )
        .unwrap();
        let Intake::HostCompletedPackets {
            command: Ok(command),
            ..
        } = endpoints
            .controller
            .try_receive_active_peripheral_with_buffer(
                ready,
                Some(ConnHandle::new(1)),
                false,
                &mut scratch,
                |_, _| panic!("occupied Host TX must not be replaced"),
            )
        else {
            panic!("credit must bypass occupied TX")
        };
        owner
            .acl
            .accept_host_completed_packets(command, Some(ConnHandle::new(1)))
            .unwrap();
        assert!(matches!(
            owner
                .acl
                .try_publish_controller_packet(&endpoints.controller, 27, true, Some(1)),
            ControllerAclPublication::Published
        ));
        let _: oer_bluetooth_hci::bt_hci::ControllerToHostPacket<'_> =
            block_on(endpoints.host.read(&mut scratch)).unwrap();
        assert!(!owner.acl.controller_credits_settled());
        assert!(owner.acl.can_reserve_controller_event());

        let prepare = |event, phase| {
            prepare_recurring_protocol_proposal(
                event,
                phase,
                None,
                PeripheralConnectionRecurrence::Maintenance(delta),
                epoch(anchor),
                SchedulerSoftwareConfig::reviewed_standalone(),
                software_policy(),
            )
        };
        let ControlFlow::Continue(candidate) = prepare(owner.event, phase(anchor)) else {
            panic!("bounded pause before Instant")
        };
        let expected = candidate.proposal;
        let (event, original_phase, _) = candidate.cancel();
        assert_eq!(event.event_counter(), 0);
        let ControlFlow::Continue(candidate) = prepare(event, original_phase) else {
            panic!("cancel does not spend the pause")
        };
        assert_eq!(candidate.proposal, expected);
        assert_eq!(
            expected.proposed_anchor.image(),
            anchor.wrapping_add(22_500)
        );
        assert_eq!(candidate.event_counter(), 3);
        let budget = PeripheralMaintenanceBudget::new(
            NonZeroU32::new(10_000).unwrap(),
            NonZeroU32::new(1_000).unwrap(),
            NonZeroU32::new(25).unwrap(),
        )
        .unwrap();
        let window = budget
            .admit(
                at(anchor),
                expected.proposed_anchor.wrapping_add(u32::MAX - 99),
                expected.proposed_anchor.wrapping_add(500),
                50_000,
                50_000,
                50,
                Deadlines {
                    termination: None,
                    procedure: None,
                    supervision: Some(PeripheralSupervisionDeadline::new(at(anchor), 100_000)),
                },
            )
            .unwrap();
        assert!(window.execution.check(Some(59_999)).is_ok());
        assert!(window.restoration.check(Some(60_999)).is_ok());
        assert!(matches!(
            window.restoration.check(Some(61_000)),
            Err(oer_esp32s31_phy::tracking::deadline::TrackingDeadlineError::Expired { .. })
        ));
        // Continue the successful, pre-deadline branch. The equality branch
        // above grants no permission to publish; physical poisoning is HIL scope.
        owner.event = candidate
            .provisional
            .commit()
            .into_submitted()
            .complete(LePeripheralConnectionEventPeerActivity::Missed);
        assert_eq!(
            owner.event.maintenance_skip_eligible(delta),
            Err(SkipBlocked::RecoveryEventRequired)
        );
        assert!(owner.acl.reserve_controller_event());
        assert!(!owner.acl.reserve_controller_event());
        assert_eq!(owner.acl.take_completed_host_packets(), 0);
        owner.acl.observe_transmission_completion(false);
        assert_eq!(owner.acl.take_completed_host_packets(), 0);
        owner.acl.observe_transmission_completion(true);
        for length in [23, 1] {
            assert_eq!(owner.acl.next_fragment(23).unwrap().payload().len(), length);
            owner.acl.fragment_enqueued(length);
            owner.acl.observe_transmission_completion(true);
        }
        assert_eq!(owner.acl.take_completed_host_packets(), 1);
        assert_eq!(owner.acl.take_completed_host_packets(), 0);
        let mut current_phase = expected.proposed_phase;
        for counter in 4..=6 {
            let ControlFlow::Continue(next) = prepare_recurring_protocol_proposal(
                owner.event,
                current_phase,
                None,
                LePeripheralConnectionEventDelta::new(1).unwrap(),
                epoch(anchor),
                SchedulerSoftwareConfig::reviewed_standalone(),
                software_policy(),
            ) else {
                panic!("recovery must preserve the scheduled Instant")
            };
            assert_eq!(next.event_counter(), counter);
            assert_eq!(
                next.proposal.proposed_anchor.image(),
                anchor.wrapping_add(7_500 * u32::from(counter))
            );
            if counter == 6 {
                assert!(next.provisional.channel().get() < 2);
            }
            current_phase = next.proposal.proposed_phase;
            owner.event = next
                .provisional
                .commit()
                .into_submitted()
                .complete(LePeripheralConnectionEventPeerActivity::Observed);
            if counter == 4 {
                assert_eq!(
                    owner.event.maintenance_skip_eligible(delta),
                    Err(SkipBlocked::RecoveryEventRequired)
                );
            }
            owner.event.observe_valid_packet_header(0x0d);
            if counter == 4 {
                assert_eq!(
                    owner.event.maintenance_skip_eligible(delta),
                    Err(SkipBlocked::InstantProcedure)
                );
            }
            owner.acl.complete_controller_event();
            if counter < 6 {
                assert!(owner.acl.reserve_controller_event());
            }
        }
        assert_eq!(owner.event.maintenance_skip_eligible(delta), Ok(()));
    }
}

#[test]
fn stalled_host_credit_does_not_renew_protocol_or_maintenance_deadlines() {
    for origin in [0, u32::MAX - 20_000_000] {
        let mut owner = completed_after_delayed_irq();
        receive_data(&mut owner, &[2, 1, 7]);
        owner.acl.complete_controller_event();
        let mut resources = hci();
        let mut endpoints = resources.split();
        let mut scratch = [0; 80];
        assert!(matches!(
            owner
                .acl
                .try_publish_controller_packet(&endpoints.controller, 27, true, Some(1)),
            ControllerAclPublication::Published
        ));
        let _: oer_bluetooth_hci::bt_hci::ControllerToHostPacket<'_> =
            block_on(endpoints.host.read(&mut scratch)).unwrap();
        owner.control.admit_remote_feature_request();
        owner.control.activate_remote_feature_request();
        owner.control.response_enqueued();
        let deadlines = Deadlines {
            termination: None,
            procedure: PeripheralProcedureDeadline::after_graph_update(
                None,
                Some(at(origin)),
                owner.control.local_feature_request_transmitted(),
                true,
            ),
            supervision: Some(PeripheralSupervisionDeadline::new(
                at(origin.wrapping_add(39_000_000)),
                2_000_000,
            )),
        };
        assert!(deadlines.procedure.is_some());
        let budget = PeripheralMaintenanceBudget::new(
            NonZeroU32::new(100).unwrap(),
            NonZeroU32::new(50).unwrap(),
            NonZeroU32::new(25).unwrap(),
        )
        .unwrap();
        assert!(owner.acl.reserve_controller_event());
        let mut completion = SingleItemCompletion::<irq::Role>::new(irq::Owners {
            event: owner
                .event
                .prepare_recurring_event(LePeripheralConnectionEventDelta::new(1).unwrap())
                .commit()
                .into_submitted(),
            acl: owner.acl,
            control: owner.control,
        });
        let mut hw = irq::Hardware::default();
        for elapsed in [39_999_000, 39_999_500, 39_999_999] {
            completion = irq::wait(completion, &mut hw);
            let now = at(origin.wrapping_add(elapsed));
            assert_eq!(
                deadlines.decide(now, at(origin.wrapping_add(40_000_000))),
                Decision::Wait
            );
            assert_eq!(
                budget
                    .admit(
                        now,
                        at(origin.wrapping_add(40_000_000)),
                        at(origin.wrapping_add(40_000_100)),
                        1_000,
                        1_000,
                        0,
                        deadlines
                    )
                    .unwrap_err(),
                PeripheralMaintenanceBlocked::ProtocolDeadline
            );
        }
        // Completion at the deadline returns ownership but does not acknowledge
        // an unanswered LL procedure or invent a fresh supervision reference.
        hw.wake = true;
        hw.finished = true;
        hw.post_unlink_ready = true;
        for _ in 0..4 {
            completion = irq::advance(completion, &mut hw);
        }
        let SingleItemCompletionStep::RemovalReady(mut owner) = completion.step(&mut hw) else {
            panic!("matching completion releases the same pending credits")
        };
        owner.acl.complete_controller_event();
        assert!(!owner.acl.controller_credits_settled());
        assert!(owner.control.local_feature_request_transmitted());
        let now = at(origin.wrapping_add(40_000_000));
        assert_eq!(
            deadlines.decide(now, now),
            Decision::Expired(Expired::Procedure { reason: 0x22 })
        );
        owner.control.expire_local_procedure();
        owner.acl.cancel_host_packet();
        assert_eq!(owner.acl.take_completed_host_packets(), 1);
        owner.acl.cancel_host_packet();
        assert_eq!(owner.acl.take_completed_host_packets(), 0);
        // Disconnected handle remains a credit identity until the old Host
        // buffer returns. No test-only mutation of the outstanding count.
        assert_eq!(owner.acl.credit_handle(None), Some(ConnHandle::new(1)));
        let LeControllerCommandReadyClaim::Ready(mut ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("command owner")
        };
        for (handle, expected_ok) in [(2, false), (1, true), (1, false)] {
            block_on(
                endpoints
                    .host
                    .acl_credit_sender()
                    .return_completed_packets(&[ConnHandleCompletedPackets::new(
                        ConnHandle::new(handle),
                        1,
                    )]),
            )
            .unwrap();
            let live = owner.acl.credit_handle(None);
            let Intake::HostCompletedPackets {
                ready: next,
                command: Ok(command),
                ..
            } = endpoints
                .controller
                .try_receive_active_peripheral_with_buffer(
                    ready,
                    live,
                    true,
                    &mut scratch,
                    |_, _| panic!("no queued data"),
                )
            else {
                panic!("credit intake")
            };
            ready = next;
            assert_eq!(
                owner
                    .acl
                    .accept_host_completed_packets(command, live)
                    .is_ok(),
                expected_ok
            );
            assert_eq!(owner.acl.controller_credits_settled(), handle == 1);
        }
        assert_eq!(owner.acl.credit_handle(None), None);
    }
}

#[test]
fn failed_head_retirement_retains_acl_and_ll_owners_without_unlink() {
    let mut hw = irq::Hardware {
        wake: true,
        finished: true,
        fail_head: true,
        ..Default::default()
    };
    let completion = irq::advance(flight(), &mut hw);
    let completion = irq::advance(completion, &mut hw);
    let SingleItemCompletionStep::Fault(mut fault) = completion.step(&mut hw) else {
        panic!("published head cannot release the role")
    };
    assert_eq!(hw.unlinks, 0);
    assert_eq!(hw.completions, 1);
    assert_eq!(fault._owner.event.event_counter(), 0);
    assert!(fault._owner.acl.controller_event_is_reserved());
    assert!(!fault._owner.acl.can_accept_host_packet());
    assert_eq!(fault._owner.acl.take_completed_host_packets(), 0);
}
