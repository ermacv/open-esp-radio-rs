//! LE peripheral connections with the runner as the HCI Host.
//!
//! The image passes raw HCI through; this module is the whole Host. It resets
//! the Controller, declares one 27-octet Host ACL buffer with
//! Controller-to-Host flow control, and advertises connectable. The Linux
//! helper connects as central on its own thread while this thread pumps the
//! Controller:
//!
//! - every received ACL fragment's credit returns explicitly, the first
//!   fragment of each L2CAP frame after a 300 ms hold, and each complete
//!   frame is echoed as one ACL packet;
//! - LE Long Term Key Requests are answered by the connection's key policy:
//!   the fixed public test keys, or a deliberate missing, wrong or missing
//!   refresh key; for an active-data MIC failure the Host arms the
//!   Controller's diagnostic MIC fault as soon as the connection completes,
//!   and no data may reach it;
//! - with PHY tracking required, the image's `PhyTracking` counters are read
//!   when the connection completes and when it ends, and periodic tracking
//!   must have completed at least one pass in between without a skip;
//! - the connection ends as its profile asks: the peer resets, loses RF or
//!   disconnects, the Controller terminates on a key failure, or the target
//!   Host disconnects or resets its Controller after both echoes.
//!
//! A diagnostic image can also end the Controller epoch between connections
//! (Reset, retirement of the Host end, powered stop and a restart on the same
//! storage) and retire the Controller after the last one; each requires both
//! directions of the old Host end to report the transport closed.
//!
//! ACL timing observed here includes the HIL console link and supports no
//! throughput or latency claim.

use super::hci::{self, Packet};
use crate::{
    Result,
    fixture::bluetooth::{
        self as fixture,
        model::{Adapter, PeerAddress},
    },
};
use hil_core::{context::Context, session::SerialCapture};
use oer_hil_protocol::{
    BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES, BLUETOOTH_REFRESH_EDIV, BLUETOOTH_REFRESH_LTK,
    BLUETOOTH_REFRESH_RAND, BLUETOOTH_TEST_EDIV, BLUETOOTH_TEST_LTK, BLUETOOTH_TEST_RAND,
    BluetoothHciLifecycle, BluetoothHciLifecycleEvidence,
    BluetoothPeripheralTermination as Termination, BluetoothSecurityFailure, PhyTrackingCommand,
    PhyTrackingEvidence,
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// Host ACL buffer the image declares: one 27-octet buffer, so the
/// Controller delivers each Link Layer fragment separately and waits for its
/// credit.
const HOST_ACL_BYTES: u16 = 27;
const HOST_ACL_BUFFERS: u16 = 1;
/// How long the Host keeps the first fragment's credit of each frame.
const CREDIT_HOLD: Duration = Duration::from_millis(300);
/// ACL fragments one echoed frame arrives in.
const FRAGMENTS_PER_FRAME: u32 =
    (BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES as u32).div_ceil(HOST_ACL_BYTES as u32);
/// Echoes the connect-reset helper requires before it ends the connection.
const ECHOES: u32 = 2;
/// Bound on one connection cycle, above the helper's own 45 s.
const CYCLE_DEADLINE: Duration = Duration::from_secs(50);
/// 100 ms advertising interval in 0.625 ms units.
const ADVERTISING_INTERVAL: u16 = 160;
/// The helper's requested connection interval, 120 ms, in 1.25 ms units.
const UPDATED_INTERVAL: u16 = 96;
/// A key the central does not hold.
const WRONG_LTK: [u8; 16] = [0xa5; 16];

const DISCONNECTION_COMPLETE: u8 = 0x05;
const ENCRYPTION_CHANGE: u8 = 0x08;
const NUMBER_OF_COMPLETED_PACKETS: u8 = 0x13;
const ENCRYPTION_KEY_REFRESH_COMPLETE: u8 = 0x30;
const LE_META: u8 = 0x3e;
const LE_CONNECTION_COMPLETE: u8 = 0x01;
const LE_CONNECTION_UPDATE_COMPLETE: u8 = 0x03;
const LE_LONG_TERM_KEY_REQUEST: u8 = 0x05;
const LE_LONG_TERM_KEY_REQUEST_REPLY: u16 = 0x201a;
const LE_LONG_TERM_KEY_REQUEST_NEGATIVE_REPLY: u16 = 0x201b;
/// The Controller's diagnostic command: corrupt the MIC of the next received
/// encrypted data PDU of one connection.
const ARM_MIC_FAULT: u16 = 0xfc01;
const KEY_MISSING: u8 = 0x06;
const SUPERVISION_TIMEOUT: u8 = 0x08;
const REMOTE_USER_TERMINATED: u8 = 0x13;
const LOCAL_HOST_TERMINATED: u8 = 0x16;
const MIC_FAILURE: u8 = 0x3d;
const PERIPHERAL_ROLE: u8 = 0x01;
/// First non-automatically-flushable packet, the LE Host's only start flag.
const HOST_START: u16 = 0b00;
const CONTROLLER_START: u16 = 0b10;
const CONTINUATION: u16 = 0b01;

/// Encryption of the connect-reset connections.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Security {
    #[default]
    Plaintext,
    /// The central encrypts with the fixed test key.
    Encrypted,
    /// The central then replaces it with the refresh key.
    KeyRefresh,
}

/// One scenario: the connections in order.
#[derive(Clone, Copy, Debug)]
pub enum Config {
    /// `connections` connect-reset cycles.
    Connections {
        connections: u8,
        hold_millis: u16,
        termination: Termination,
        security: Security,
        phy_tracking: bool,
        /// Restart the Controller epoch between connections.
        restart_between_connections: bool,
        /// Retire the Controller after the last connection.
        retire_after: bool,
    },
    /// One connection whose encryption fails as `failure` asks, then one
    /// encrypted connect-reset connection that must succeed.
    SecurityFailure {
        failure: BluetoothSecurityFailure,
        read_version_before_disconnect: bool,
    },
}

/// What the Host answers to the central's key requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Keys {
    /// No key request may arrive.
    None,
    /// The test key, and the refresh key when the central refreshes.
    Valid,
    /// The first request is refused.
    Missing,
    /// The first request gets a key the central does not hold.
    Wrong,
    /// The test key, then the refresh request is refused.
    MissingRefresh,
}

/// How one connection must go, from the Host's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Profile {
    echoes: u32,
    update: bool,
    keys: Keys,
    refreshes: u32,
    local: Option<Termination>,
    reasons: &'static [u8],
    /// Periodic PHY tracking must complete a pass during the connection.
    phy_tracking: bool,
    /// Arm the Controller's MIC fault when the connection completes.
    mic_fault: bool,
}

impl Profile {
    fn connect_reset(termination: Termination, security: Security) -> Self {
        let local = matches!(
            termination,
            Termination::TargetDisconnect | Termination::TargetReset
        )
        .then_some(termination);
        Self {
            echoes: ECHOES,
            update: true,
            keys: match security {
                Security::Plaintext => Keys::None,
                Security::Encrypted | Security::KeyRefresh => Keys::Valid,
            },
            refreshes: u32::from(security == Security::KeyRefresh),
            local,
            phy_tracking: false,
            mic_fault: false,
            reasons: match termination {
                Termination::PeerReset => &[SUPERVISION_TIMEOUT],
                Termination::PeerRfkill => &[SUPERVISION_TIMEOUT, REMOTE_USER_TERMINATED],
                Termination::TargetDisconnect => &[LOCAL_HOST_TERMINATED],
                Termination::TargetReset => &[],
            },
        }
    }

    fn security_failure(failure: BluetoothSecurityFailure) -> Result<Self> {
        let (keys, reasons, mic_fault): (Keys, &'static [u8], bool) = match failure {
            // The link survives the refusal until the central disconnects.
            BluetoothSecurityFailure::MissingKey => {
                (Keys::Missing, &[REMOTE_USER_TERMINATED], false)
            }
            // The central's encrypted response fails authentication here.
            BluetoothSecurityFailure::WrongKey => (Keys::Wrong, &[MIC_FAILURE], false),
            BluetoothSecurityFailure::MissingRefreshKey => {
                (Keys::MissingRefresh, &[KEY_MISSING], false)
            }
            // The central's first encrypted data fails authentication here.
            BluetoothSecurityFailure::ActiveDataMic => (Keys::Valid, &[MIC_FAILURE], true),
        };
        Ok(Self {
            echoes: 0,
            update: false,
            keys,
            refreshes: 0,
            local: None,
            reasons,
            phy_tracking: false,
            mic_fault,
        })
    }
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("set [bluetooth] adapter in the local lab configuration")?;
    context.with_capture(output, |capture| {
        let mut report = Report {
            schema: 1,
            adapter: adapter.to_string(),
            scenario: format!("{config:?}"),
            address: None,
            cycles: Vec::new(),
            lifecycle: Vec::new(),
            retired: false,
            passed: false,
            error: None,
            cleanup_error: None,
        };
        let result = exercise(capture, adapter, config, output, &mut report);
        // A retired Controller serves no further command.
        let cleanup = oer_process::cleanup(|| {
            if report.retired {
                Ok(())
            } else {
                hci::command(capture, hci::RESET, &[]).map(|_| ())
            }
        });
        report.passed = result.is_ok() && cleanup.is_ok();
        report.error = result.as_ref().err().map(ToString::to_string);
        report.cleanup_error = cleanup.as_ref().err().map(ToString::to_string);
        hil_core::durable::atomic_json(&output.join("bluetooth-peripheral.json"), &report)?;
        result.and(cleanup)
    })
}

/// One helper invocation as central.
#[derive(Clone, Copy, Debug)]
enum Central {
    ConnectReset {
        hold_millis: u16,
        termination: Termination,
        security: Security,
    },
    SecurityFailure {
        failure: BluetoothSecurityFailure,
        read_version_before_disconnect: bool,
    },
}

impl Central {
    fn run(self, directory: &Path, adapter: Adapter, peer: PeerAddress) -> Result<()> {
        match self {
            Self::ConnectReset {
                hold_millis,
                termination,
                security,
            } => fixture::connect_profile_in(
                directory,
                adapter,
                peer,
                hold_millis,
                termination,
                security != Security::Plaintext,
                security == Security::KeyRefresh,
            )
            .map(|_| ()),
            Self::SecurityFailure {
                failure,
                read_version_before_disconnect,
            } => fixture::security_failure_in(
                directory,
                adapter,
                peer,
                failure,
                read_version_before_disconnect,
            )
            .map(|_| ()),
        }
    }
}

fn plan(config: Config) -> Result<Vec<(Central, Profile)>> {
    Ok(match config {
        Config::Connections {
            connections,
            hold_millis,
            termination,
            security,
            phy_tracking,
            ..
        } => (0..connections)
            .map(|_| {
                (
                    Central::ConnectReset {
                        hold_millis,
                        termination,
                        security,
                    },
                    Profile {
                        phy_tracking,
                        ..Profile::connect_reset(termination, security)
                    },
                )
            })
            .collect(),
        Config::SecurityFailure {
            failure,
            read_version_before_disconnect,
        } => vec![
            (
                Central::SecurityFailure {
                    failure,
                    read_version_before_disconnect,
                },
                Profile::security_failure(failure)?,
            ),
            (
                Central::ConnectReset {
                    hold_millis: 100,
                    termination: Termination::PeerReset,
                    security: Security::Encrypted,
                },
                Profile::connect_reset(Termination::PeerReset, Security::Encrypted),
            ),
        ],
    })
}

fn exercise(
    capture: &SerialCapture,
    adapter: Adapter,
    config: Config,
    output: &Path,
    report: &mut Report,
) -> Result<()> {
    hci::require(capture)?;
    let (restart_between, retire_after) = match config {
        Config::Connections {
            restart_between_connections,
            retire_after,
            ..
        } => (restart_between_connections, retire_after),
        Config::SecurityFailure { .. } => (false, false),
    };
    if (restart_between || retire_after)
        && !capture
            .request_capabilities(Duration::from_secs(10))?
            .features
            .bluetooth_hci_lifecycle
    {
        return Err("firmware lacks Controller epoch restart and retirement".into());
    }
    let host = Host::start(capture)?;
    report.address = Some(host.address.to_string());
    let plan = plan(config)?;
    let last = plan.len();
    for (index, (central, profile)) in plan.into_iter().enumerate() {
        let directory = output.join(format!("connection-{:03}", index + 1));
        host.advertise()?;
        let mut observed = Cycle {
            keys: profile.keys,
            ..Cycle::default()
        };
        let result = std::thread::scope(|scope| {
            let helper = scope.spawn(|| central.run(&directory, adapter, host.address));
            let served = host.serve(profile, &mut observed, || helper.is_finished());
            let helper = helper
                .join()
                .map_err(|_| "Bluetooth helper thread panicked")?;
            served.and(helper)
        });
        observed.error = result.as_ref().err().map(ToString::to_string);
        report.cycles.push(observed);
        result.map_err(|error| format!("connection {}: {error}", index + 1))?;
        if restart_between && index + 1 < last {
            let evidence = hci::lifecycle(capture, BluetoothHciLifecycle::Restart)?;
            report.lifecycle.push(evidence);
            ended(BluetoothHciLifecycle::Restart, evidence)?;
            host.initialize()?;
        } else if profile.local == Some(Termination::TargetReset) {
            host.initialize()?;
        }
    }
    if retire_after {
        let evidence = hci::lifecycle(capture, BluetoothHciLifecycle::Retire)?;
        report.lifecycle.push(evidence);
        report.retired = true;
        ended(BluetoothHciLifecycle::Retire, evidence)?;
    }
    Ok(())
}

/// The epoch ended cleanly: the old Host end is closed both ways, and a
/// restart started the next epoch while a retirement did not.
fn ended(operation: BluetoothHciLifecycle, evidence: BluetoothHciLifecycleEvidence) -> Result<()> {
    let restarted = operation == BluetoothHciLifecycle::Restart;
    if evidence.old_host_closed && evidence.restarted == restarted {
        Ok(())
    } else {
        Err(format!("Controller {operation:?} ended as {evidence:?}").into())
    }
}

/// The Host end of one image's Controller.
pub(super) struct Host<'a> {
    capture: &'a SerialCapture,
    pub(super) address: PeerAddress,
}

impl<'a> Host<'a> {
    pub(super) fn start(capture: &'a SerialCapture) -> Result<Self> {
        let host = Self {
            capture,
            address: PeerAddress([0; 6]),
        };
        host.initialize()?;
        let address: [u8; 6] = hci::command(capture, hci::READ_BD_ADDR, &[])?
            .try_into()
            .map_err(|_| "Read BD_ADDR returned no address")?;
        Ok(Self {
            address: PeerAddress(address),
            ..host
        })
    }

    /// Reset the Controller and configure flow control; the Controller's
    /// own ACL buffers must carry one whole frame.
    pub(super) fn initialize(&self) -> Result<()> {
        let capture = self.capture;
        hci::command(capture, hci::RESET, &[])?;
        hci::command(capture, hci::SET_EVENT_MASK, &hci::EVENT_MASK_WITH_LE_META)?;
        let buffers = hci::command(capture, hci::LE_READ_BUFFER_SIZE, &[])?;
        let length = buffers
            .get(0..2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
            .ok_or("LE Read Buffer Size returned no length")?;
        if usize::from(length) < BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES
            || buffers.get(2) == Some(&0)
        {
            return Err(
                format!("Controller ACL buffers {buffers:02x?} cannot carry one frame").into(),
            );
        }
        let mut host_buffers = [0; 7];
        host_buffers[0..2].copy_from_slice(&HOST_ACL_BYTES.to_le_bytes());
        host_buffers[3..5].copy_from_slice(&HOST_ACL_BUFFERS.to_le_bytes());
        hci::command(capture, hci::HOST_BUFFER_SIZE, &host_buffers)?;
        hci::command(capture, hci::SET_CONTROLLER_TO_HOST_FLOW_CONTROL, &[0x01])?;
        Ok(())
    }

    /// Connectable undirected advertising from the public address.
    pub(super) fn advertise(&self) -> Result<()> {
        let mut parameters = [0; 15];
        parameters[0..2].copy_from_slice(&ADVERTISING_INTERVAL.to_le_bytes());
        parameters[2..4].copy_from_slice(&ADVERTISING_INTERVAL.to_le_bytes());
        parameters[13] = 0x07;
        hci::command(
            self.capture,
            hci::LE_SET_ADVERTISING_PARAMETERS,
            &parameters,
        )?;
        hci::command(
            self.capture,
            hci::LE_SET_ADVERTISING_DATA,
            &hci::data(&[0x02, 0x01, 0x06])?,
        )?;
        hci::command(self.capture, hci::LE_SET_ADVERTISING_ENABLE, &[1])?;
        Ok(())
    }

    /// Pump one connection until it ended as `profile` asks and the helper
    /// has finished.
    fn serve(
        &self,
        profile: Profile,
        cycle: &mut Cycle,
        helper_finished: impl Fn() -> bool,
    ) -> Result<()> {
        let started = Instant::now();
        let mut link = Link::default();
        loop {
            if started.elapsed() > CYCLE_DEADLINE {
                return Err("the connection cycle exceeded its deadline".into());
            }
            if let Some(handle) = link.handle
                && let Some(until) = link.held_until
                && Instant::now() >= until
            {
                hci::return_credits(self.capture, handle, 1)?;
                link.held_until = None;
                cycle.credits_returned += 1;
            }
            if link.ended && helper_finished() {
                return link.finish(profile, cycle);
            }
            // A local termination waits until the Controller has sent the
            // second echo, so that the peer receives it first.
            if let Some(local) = profile.local
                && link.echoes == profile.echoes
                && !link.controller_busy
                && !link.terminated_locally
            {
                self.terminate(&mut link, local)?;
            }
            let wait = if link.held_until.is_some() {
                Duration::from_millis(20)
            } else {
                Duration::from_millis(200)
            };
            let Some(packet) = hci::next_packet(self.capture, wait)? else {
                if link.ended && !helper_finished() {
                    std::thread::sleep(Duration::from_millis(50));
                }
                continue;
            };
            match packet {
                Packet::Event(event) => {
                    let connected = link.handle.is_some();
                    let reply = link.event(&event, profile, cycle)?;
                    if profile.phy_tracking && (link.handle.is_some() != connected || link.ended) {
                        let tracking = self.capture.phy_tracking(PhyTrackingCommand::Status)?;
                        if connected {
                            cycle.tracking_after.get_or_insert(tracking);
                        } else {
                            cycle.tracking_before = Some(tracking);
                        }
                    }
                    if let Some(reply) = reply {
                        self.answer(&link, reply)?;
                    }
                    if profile.mic_fault
                        && !link.mic_armed
                        && let Some(handle) = link.handle
                    {
                        self.arm_mic_fault(handle)?;
                        link.mic_armed = true;
                        cycle.mic_armed = true;
                    }
                }
                Packet::Acl(packet) => {
                    if profile.mic_fault {
                        return Err("data reached the Host despite the corrupted MIC".into());
                    }
                    let (handle, frame) = link.acl(&packet, cycle)?;
                    if link.held_until.is_none() {
                        hci::return_credits(self.capture, handle, 1)?;
                        cycle.credits_returned += 1;
                    }
                    if let Some(frame) = frame {
                        self.echo(&mut link, profile, handle, &frame, cycle)?;
                    }
                }
            }
        }
    }

    fn terminate(&self, link: &mut Link, local: Termination) -> Result<()> {
        match local {
            Termination::TargetDisconnect => {
                let handle = link.handle.ok_or("echoes without a connection")?;
                let [low, high] = handle.to_le_bytes();
                let status = hci::command_status(
                    self.capture,
                    hci::DISCONNECT,
                    &[low, high, REMOTE_USER_TERMINATED],
                )?;
                if status != 0 {
                    return Err(format!("Disconnect returned status {status:#04x}").into());
                }
            }
            Termination::TargetReset => {
                hci::command(self.capture, hci::RESET, &[])?;
                link.ended = true;
            }
            Termination::PeerReset | Termination::PeerRfkill => {}
        }
        link.terminated_locally = true;
        Ok(())
    }

    /// Corrupt the MIC of the next encrypted data PDU `handle` receives.
    fn arm_mic_fault(&self, handle: u16) -> Result<()> {
        let returned = hci::command(self.capture, ARM_MIC_FAULT, &handle.to_le_bytes())?;
        if returned != handle.to_le_bytes() {
            return Err(format!("the MIC fault command returned {returned:02x?}").into());
        }
        Ok(())
    }

    /// Answer one LE Long Term Key Request.
    fn answer(&self, link: &Link, reply: KeyReply) -> Result<()> {
        let handle = link.handle.ok_or("key request without a connection")?;
        let [low, high] = handle.to_le_bytes();
        let returned = match reply {
            KeyReply::Key(key) => {
                let mut parameters = [0; 18];
                parameters[0..2].copy_from_slice(&[low, high]);
                parameters[2..].copy_from_slice(&key);
                hci::command(self.capture, LE_LONG_TERM_KEY_REQUEST_REPLY, &parameters)?
            }
            KeyReply::Negative => hci::command(
                self.capture,
                LE_LONG_TERM_KEY_REQUEST_NEGATIVE_REPLY,
                &[low, high],
            )?,
        };
        if returned != [low, high] {
            return Err(format!("the key reply returned {returned:02x?}").into());
        }
        Ok(())
    }

    /// Send one frame back as one ACL packet once the Controller has a
    /// buffer for it.
    fn echo(
        &self,
        link: &mut Link,
        profile: Profile,
        handle: u16,
        frame: &[u8],
        cycle: &mut Cycle,
    ) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while link.controller_busy {
            if Instant::now() > deadline {
                return Err("the Controller returned no ACL buffer for the echo".into());
            }
            match hci::next_packet(self.capture, Duration::from_millis(200))? {
                Some(Packet::Event(event)) => {
                    if let Some(reply) = link.event(&event, profile, cycle)? {
                        self.answer(link, reply)?;
                    }
                }
                Some(Packet::Acl(_)) => {
                    return Err("ACL data arrived before the previous echo completed".into());
                }
                None => {}
            }
        }
        let mut packet = Vec::with_capacity(4 + frame.len());
        packet.extend_from_slice(&(handle | (HOST_START << 12)).to_le_bytes());
        packet.extend_from_slice(&(frame.len() as u16).to_le_bytes());
        packet.extend_from_slice(frame);
        hci::acl(self.capture, &packet)?;
        link.controller_busy = true;
        link.echoes += 1;
        cycle.echoes += 1;
        Ok(())
    }
}

/// The Host's answer to one key request.
#[derive(Debug, PartialEq, Eq)]
enum KeyReply {
    Key([u8; 16]),
    Negative,
}

/// Host state of one connection.
#[derive(Default)]
struct Link {
    handle: Option<u16>,
    frame: Vec<u8>,
    fragments: u32,
    held_until: Option<Instant>,
    /// An echo occupies the Controller's buffer until its completion.
    controller_busy: bool,
    echoes: u32,
    key_requests: u32,
    encrypted: bool,
    refreshes: u32,
    mic_armed: bool,
    terminated_locally: bool,
    ended: bool,
    reason: Option<u8>,
}

impl Link {
    fn event(
        &mut self,
        event: &[u8],
        profile: Profile,
        cycle: &mut Cycle,
    ) -> Result<Option<KeyReply>> {
        let parameters = event.get(2..).unwrap_or_default();
        match (event.first().copied(), parameters.first().copied()) {
            (Some(LE_META), Some(LE_CONNECTION_COMPLETE)) => {
                // Subevent, status, handle, role, peer kind, peer, interval.
                if parameters.len() < 18 {
                    return Err(format!("short LE Connection Complete {event:02x?}").into());
                }
                if self.handle.is_some() {
                    return Err("a second connection completed on the same cycle".into());
                }
                if parameters[1] != 0 || parameters[4] != PERIPHERAL_ROLE {
                    return Err(format!("the connection failed: {event:02x?}").into());
                }
                let handle = u16::from_le_bytes([parameters[2], parameters[3]]);
                self.handle = Some(handle);
                cycle.handle = Some(handle);
                cycle.central =
                    Some(PeerAddress(parameters[6..12].try_into().expect("six")).to_string());
                cycle.interval = Some(u16::from_le_bytes([parameters[12], parameters[13]]));
            }
            (Some(LE_META), Some(LE_CONNECTION_UPDATE_COMPLETE)) => {
                if parameters.len() < 10 {
                    return Err(format!("short LE Connection Update Complete {event:02x?}").into());
                }
                self.require_handle(parameters[2], parameters[3], event)?;
                if parameters[1] != 0 {
                    return Err(format!("the connection update failed: {event:02x?}").into());
                }
                cycle.updated_interval = Some(u16::from_le_bytes([parameters[4], parameters[5]]));
            }
            (Some(LE_META), Some(LE_LONG_TERM_KEY_REQUEST)) => {
                // Subevent, handle, random number, diversifier.
                if parameters.len() < 13 {
                    return Err(format!("short LE Long Term Key Request {event:02x?}").into());
                }
                self.require_handle(parameters[1], parameters[2], event)?;
                let rand: [u8; 8] = parameters[3..11].try_into().expect("eight");
                let ediv = u16::from_le_bytes([parameters[11], parameters[12]]);
                self.key_requests += 1;
                cycle.key_requests += 1;
                return key_reply(profile.keys, self.encrypted, rand, ediv).map(Some);
            }
            (Some(ENCRYPTION_CHANGE), _) => {
                // Status, handle, enabled.
                if parameters.len() < 4 {
                    return Err(format!("short Encryption Change {event:02x?}").into());
                }
                self.require_handle(parameters[1], parameters[2], event)?;
                cycle
                    .encryption_changes
                    .push((parameters[0], parameters[3]));
                // A failed start is reported by termination, never by a
                // failed Encryption Change.
                if parameters[0] != 0 || parameters[3] == 0 {
                    return Err(format!("unexpected Encryption Change {event:02x?}").into());
                }
                if self.encrypted {
                    return Err("encryption started twice".into());
                }
                self.encrypted = true;
            }
            (Some(ENCRYPTION_KEY_REFRESH_COMPLETE), _) => {
                // Status, handle.
                if parameters.len() < 3 {
                    return Err(
                        format!("short Encryption Key Refresh Complete {event:02x?}").into(),
                    );
                }
                self.require_handle(parameters[1], parameters[2], event)?;
                cycle.refreshes.push(parameters[0]);
                if parameters[0] == 0 {
                    self.refreshes += 1;
                }
            }
            (Some(NUMBER_OF_COMPLETED_PACKETS), Some(count)) => {
                for index in 0..usize::from(count) {
                    let entry = parameters
                        .get(1 + index * 4..5 + index * 4)
                        .ok_or("short Number Of Completed Packets")?;
                    if Some(u16::from_le_bytes([entry[0], entry[1]])) == self.handle
                        && entry[2..4] != [0, 0]
                    {
                        self.controller_busy = false;
                    }
                }
            }
            (Some(DISCONNECTION_COMPLETE), _) => {
                // Status, handle, reason.
                if parameters.len() < 4 || parameters[0] != 0 {
                    return Err(format!("unexpected Disconnection Complete {event:02x?}").into());
                }
                self.require_handle(parameters[1], parameters[2], event)?;
                self.reason = Some(parameters[3]);
                cycle.reason = Some(parameters[3]);
                self.ended = true;
            }
            _ => cycle.other_events += 1,
        }
        Ok(None)
    }

    fn require_handle(&self, low: u8, high: u8, event: &[u8]) -> Result<()> {
        if Some(u16::from_le_bytes([low, high])) == self.handle {
            Ok(())
        } else {
            Err(format!("event {event:02x?} names another connection").into())
        }
    }

    /// Consume one ACL fragment; returns its handle and the frame it
    /// completed.
    fn acl(&mut self, packet: &[u8], cycle: &mut Cycle) -> Result<(u16, Option<Vec<u8>>)> {
        let header = packet.get(0..4).ok_or("short ACL packet")?;
        let raw = u16::from_le_bytes([header[0], header[1]]);
        let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
        let data = &packet[4..];
        let handle = raw & 0x0fff;
        if data.len() != length || Some(handle) != self.handle {
            return Err(format!("ACL fragment {header:02x?} does not match the connection").into());
        }
        if data.len() > usize::from(HOST_ACL_BYTES) {
            return Err("the Controller exceeded the Host ACL buffer".into());
        }
        cycle.fragments += 1;
        match raw >> 12 & 0b11 {
            CONTROLLER_START if self.frame.is_empty() => {
                self.fragments = 0;
                self.held_until = Some(Instant::now() + CREDIT_HOLD);
                cycle.credits_held += 1;
            }
            CONTINUATION if !self.frame.is_empty() => {}
            flag => return Err(format!("ACL fragment boundary {flag:#04b} out of order").into()),
        }
        self.frame.extend_from_slice(data);
        self.fragments += 1;
        let expected = self
            .frame
            .get(0..2)
            .map(|length| 4 + usize::from(u16::from_le_bytes([length[0], length[1]])));
        match expected {
            Some(expected) if self.frame.len() > expected => {
                Err("ACL fragments exceed their L2CAP length".into())
            }
            Some(expected) if self.frame.len() == expected => {
                if self.fragments != FRAGMENTS_PER_FRAME {
                    return Err(format!(
                        "a frame arrived in {} fragments, not {FRAGMENTS_PER_FRAME}",
                        self.fragments
                    )
                    .into());
                }
                Ok((handle, Some(std::mem::take(&mut self.frame))))
            }
            _ => Ok((handle, None)),
        }
    }

    fn finish(&self, profile: Profile, cycle: &Cycle) -> Result<()> {
        if self.echoes != profile.echoes {
            return Err(format!(
                "{} of {} echoes before the connection ended",
                self.echoes, profile.echoes
            )
            .into());
        }
        if profile.update && cycle.updated_interval != Some(UPDATED_INTERVAL) {
            return Err(format!(
                "the connection update to {UPDATED_INTERVAL} was not observed: {:?}",
                cycle.updated_interval
            )
            .into());
        }
        let (encrypted, requests) = match profile.keys {
            Keys::None => (false, 0),
            Keys::Valid => (true, 1 + profile.refreshes),
            Keys::Missing | Keys::Wrong => (false, 1),
            Keys::MissingRefresh => (true, 2),
        };
        if self.encrypted != encrypted || self.key_requests != requests {
            return Err(format!(
                "encryption {} after {} key requests, expected {encrypted} after {requests}",
                self.encrypted, self.key_requests
            )
            .into());
        }
        if self.refreshes != profile.refreshes {
            return Err(format!(
                "{} key refreshes, expected {}",
                self.refreshes, profile.refreshes
            )
            .into());
        }
        if profile.phy_tracking {
            tracked_during(cycle.tracking_before, cycle.tracking_after)?;
        }
        if profile.mic_fault != self.mic_armed {
            return Err("the MIC fault was not armed as the profile asks".into());
        }
        match self.reason {
            Some(reason) if profile.reasons.contains(&reason) => Ok(()),
            None if profile.reasons.is_empty() => Ok(()),
            reason => Err(format!("the connection ended with reason {reason:?}").into()),
        }
    }
}

/// Periodic tracking ran to completion at least once between the two
/// readings, skipped no tick and was never suspended.
fn tracked_during(
    before: Option<PhyTrackingEvidence>,
    after: Option<PhyTrackingEvidence>,
) -> Result<()> {
    let (Some(before), Some(after)) = (before, after) else {
        return Err("PHY tracking was not read at both ends of the connection".into());
    };
    if !before.running
        || !after.running
        || after.tracked <= before.tracked
        || after.skipped != before.skipped
    {
        return Err(format!(
            "PHY tracking did not complete a pass during the connection: {before:?} -> {after:?}"
        )
        .into());
    }
    Ok(())
}

/// The reply `keys` gives to a request for the key identified by `rand` and
/// `ediv`, once encryption is or is not already on.
fn key_reply(keys: Keys, encrypted: bool, rand: [u8; 8], ediv: u16) -> Result<KeyReply> {
    let test = rand == BLUETOOTH_TEST_RAND && ediv == BLUETOOTH_TEST_EDIV;
    let refresh = rand == BLUETOOTH_REFRESH_RAND && ediv == BLUETOOTH_REFRESH_EDIV;
    Ok(match (keys, encrypted, test, refresh) {
        (Keys::Valid | Keys::MissingRefresh, false, true, false) => {
            KeyReply::Key(BLUETOOTH_TEST_LTK)
        }
        (Keys::Valid, true, false, true) => KeyReply::Key(BLUETOOTH_REFRESH_LTK),
        (Keys::MissingRefresh, true, false, true) | (Keys::Missing, false, true, false) => {
            KeyReply::Negative
        }
        (Keys::Wrong, false, true, false) => KeyReply::Key(WRONG_LTK),
        _ => {
            return Err(format!(
                "unexpected key request (rand {rand:02x?}, ediv {ediv:#06x}) under {keys:?}"
            )
            .into());
        }
    })
}

#[derive(Serialize)]
struct Report {
    schema: u8,
    adapter: String,
    scenario: String,
    address: Option<String>,
    cycles: Vec<Cycle>,
    lifecycle: Vec<BluetoothHciLifecycleEvidence>,
    retired: bool,
    passed: bool,
    error: Option<String>,
    cleanup_error: Option<String>,
}

/// What the Host observed on one connection.
#[derive(Debug, Serialize)]
struct Cycle {
    keys: Keys,
    handle: Option<u16>,
    central: Option<String>,
    interval: Option<u16>,
    updated_interval: Option<u16>,
    fragments: u32,
    credits_held: u32,
    credits_returned: u32,
    echoes: u32,
    key_requests: u32,
    /// Status and enabled level of each Encryption Change.
    encryption_changes: Vec<(u8, u8)>,
    /// Status of each Encryption Key Refresh Complete.
    refreshes: Vec<u8>,
    reason: Option<u8>,
    other_events: u32,
    tracking_before: Option<PhyTrackingEvidence>,
    tracking_after: Option<PhyTrackingEvidence>,
    mic_armed: bool,
    error: Option<String>,
}

impl Default for Cycle {
    fn default() -> Self {
        Self {
            keys: Keys::None,
            handle: None,
            central: None,
            interval: None,
            updated_interval: None,
            fragments: 0,
            credits_held: 0,
            credits_returned: 0,
            echoes: 0,
            key_requests: 0,
            encryption_changes: Vec::new(),
            refreshes: Vec::new(),
            reason: None,
            other_events: 0,
            tracking_before: None,
            tracking_after: None,
            mic_armed: false,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests;
