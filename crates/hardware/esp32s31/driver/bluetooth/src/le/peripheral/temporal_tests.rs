//! Deterministic sequences using the real HCI queue, ACL owner, LL connection
//! and chip admission. No MMIO/IRQ delivery or RF quality is simulated here.
use super::{
    active_acl::PeripheralConnectionAcl,
    deadlines::{Deadlines, Decision, Expired},
    maintenance::{PeripheralMaintenanceBlocked, PeripheralMaintenanceBudget},
    supervision::PeripheralSupervisionDeadline,
};
use crate::SchedulerInstant;
use core::num::NonZeroU32;
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_bluetooth_hci::{
    BluetoothPublicDeviceAddress, LeControllerActivePeripheralIntake as Intake,
    LeControllerBootstrapConfig, LeControllerCommandReadyClaim, LeControllerHciResources,
    bt_hci::{
        data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
        param::ConnHandle,
        transport::Transport,
    },
};
use oer_bluetooth_ll::connection::{
    LeChannelSelectionAlgorithm, LeDataChannelMap, LeLegacyConnectionRequest,
    LePeripheralConnection, LePeripheralConnectionEventDelta as Delta,
    LePeripheralConnectionEventPeerActivity as Activity, maintenance::SkipBlocked,
};

fn connection() -> LePeripheralConnection {
    let mut bytes = [0u8; 36];
    bytes[0] = 0x25;
    bytes[1] = 34;
    bytes[2..14].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    bytes[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    bytes[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
    bytes[21] = 2;
    bytes[22..24].copy_from_slice(&1u16.to_le_bytes());
    bytes[24..26].copy_from_slice(&24u16.to_le_bytes());
    bytes[28..30].copy_from_slice(&200u16.to_le_bytes());
    bytes[30..35].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
    bytes[35] = 5;
    LePeripheralConnection::from_request(
        LeLegacyConnectionRequest::decode(&bytes).unwrap(),
        LeChannelSelectionAlgorithm::AlgorithmTwo,
    )
}

#[test]
fn saturated_acl_instant_pause_and_late_acquisition_preserve_the_same_owners() {
    let n = |value| NonZeroU32::new(value).unwrap();
    let budget = PeripheralMaintenanceBudget::new(n(100), n(50), n(25)).unwrap();
    for controller_origin in [1000u32, u32::MAX - 500] {
        for delayed in [false, true] {
            let mut resources = LeControllerHciResources::<NoopRawMutex, 4, 4, 258>::new(
                LeControllerBootstrapConfig::new(
                    BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
                    251,
                    4,
                )
                .unwrap(),
            )
            .unwrap();
            let mut endpoints = resources.split();
            for sequence in 1..=4 {
                block_on(endpoints.host.write(&AclPacket::new(
                    ConnHandle::new(1),
                    AclPacketBoundary::FirstNonFlushable,
                    AclBroadcastFlag::PointToPoint,
                    &[sequence; 27],
                )))
                .unwrap();
            }
            let LeControllerCommandReadyClaim::Ready(ready) = endpoints
                .controller
                .claim_initial_command_ready(PeripheralConnectionAcl::new())
            else {
                panic!("initial epoch");
            };
            let mut scratch = [0; 258];
            let Intake::Acl { ready, .. } = endpoints
                .controller
                .try_receive_active_peripheral_with_buffer(
                    ready,
                    Some(ConnHandle::new(1)),
                    true,
                    &mut scratch,
                    |mut acl, packet| {
                        acl.accept_host_packet(packet);
                        acl.fragment_enqueued(27);
                        acl
                    },
                )
            else {
                panic!("first ACL");
            };
            let Intake::Empty { ready, .. } = endpoints
                .controller
                .try_receive_active_peripheral_with_buffer(
                    ready,
                    Some(ConnHandle::new(1)),
                    false,
                    &mut scratch,
                    |_, _| panic!("maintenance must not drain an occupied ACL owner"),
                )
            else {
                panic!("three queued ACLs remain");
            };
            let ready = ready.map_owner(|mut acl| {
                let mut completed = connection()
                    .prepare_event()
                    .into_submitted()
                    .complete(Activity::Observed);
                completed.observe_valid_packet_header(0x05);
                completed
                    .schedule_channel_map_update(LeDataChannelMap::all(), 6)
                    .unwrap();
                completed.observe_valid_packet_header(0x05);
                let pause = Delta::from_skipped(1).unwrap();
                assert_eq!(
                    completed.maintenance_skip_eligible(pause),
                    Err(SkipBlocked::InstantAcknowledgement)
                );
                // Retransmission neither completes the ACL nor proves the peer
                // accepted our acknowledgement of its Instant indication.
                acl.observe_transmission_completion(false);
                completed.observe_valid_packet_header(0x05);
                assert_eq!(acl.take_completed_host_packets(), 0);
                assert_eq!(
                    completed.maintenance_skip_eligible(pause),
                    Err(SkipBlocked::InstantAcknowledgement)
                );
                completed.observe_valid_packet_header(0x0d);
                let candidate = completed.prepare_maintenance_event(pause).unwrap();
                let anchor_counter = candidate.event_counter();
                let at =
                    |delta| SchedulerInstant::from_image(controller_origin.wrapping_add(delta));
                let deadlines = Deadlines {
                    termination: None,
                    procedure: None,
                    supervision: Some(PeripheralSupervisionDeadline::new(at(0), 3000)),
                };
                let admitted = budget.admit(
                    at(0),
                    at(1000),
                    at(1100),
                    10_000,
                    if delayed { 10_800 } else { 10_010 },
                    50,
                    deadlines,
                );
                if delayed {
                    assert_eq!(
                        admitted.unwrap_err(),
                        PeripheralMaintenanceBlocked::WindowTooShort
                    );
                    let returned = candidate.cancel();
                    assert!(returned.maintenance_skip_eligible(pause).is_ok());
                    assert_eq!(
                        returned
                            .prepare_maintenance_event(pause)
                            .unwrap()
                            .event_counter(),
                        anchor_counter
                    );
                    assert_eq!(
                        deadlines.decide(at(3000), at(3100)),
                        Decision::Expired(Expired::Supervision)
                    );
                    acl.cancel_host_packet();
                } else {
                    let window = admitted.unwrap();
                    assert_eq!(window.restoration.expires_at_micros(), 10_160);
                    let mut completed = candidate
                        .commit()
                        .into_submitted()
                        .complete(Activity::Observed);
                    assert_eq!(
                        completed.maintenance_skip_eligible(pause),
                        Err(SkipBlocked::RecoveryEventRequired)
                    );
                    assert_eq!(acl.take_completed_host_packets(), 0);
                    // Only validated packet progress releases the mandatory
                    // recovery event, independently of elapsed maintenance time.
                    completed.observe_valid_packet_header(0x05);
                    assert!(completed.maintenance_skip_eligible(pause).is_ok());
                    acl.observe_transmission_completion(true);
                }
                assert_eq!(acl.take_completed_host_packets(), 1);
                assert_eq!(acl.take_completed_host_packets(), 0);
                acl
            });
            let mut ready = ready;
            for sequence in 2..=4 {
                let Intake::Acl { ready: next, .. } = endpoints
                    .controller
                    .try_receive_active_peripheral_with_buffer(
                        ready,
                        Some(ConnHandle::new(1)),
                        true,
                        &mut scratch,
                        |mut acl, packet| {
                            acl.accept_host_packet(packet);
                            assert_eq!(acl.next_fragment(27).unwrap().payload(), &[sequence; 27]);
                            acl.cancel_host_packet();
                            assert_eq!(acl.take_completed_host_packets(), 1);
                            acl
                        },
                    )
                else {
                    panic!("retained FIFO");
                };
                ready = next;
            }
            assert!(ready.owner().can_accept_host_packet());
        }
    }
}
