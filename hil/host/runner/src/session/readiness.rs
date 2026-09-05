use super::*;
use crate::execution::context::Context;

pub(super) fn session_ready_covers(
    configured: Direction,
    reported: SessionReady,
    expected: Direction,
    requirements: SessionLinkRequirements,
) -> bool {
    let direction_covers = reported.direction == expected
        || (configured == Direction::Bidirectional
            && reported.direction == Direction::Bidirectional);
    let requirements_met =
        expected != Direction::Tx || reported.tx_block_ack_tid == requirements.tx_block_ack_tid;
    direction_covers && requirements_met
}

/// Wait for a runtime-configured TCP receive service and its current IPv4
/// address. Unlike UDP, readiness does not inject a probe connection: the
/// target begins listening only after the session `Start` transition, and the
/// measured host connection is the sole stream owned by that session.
pub(crate) fn await_tcp_ready(
    capture: &SerialCapture,
    context: &Context<'_>,
    address_hint: Ipv4Addr,
    port: u16,
    direction: Direction,
    timeout: Duration,
) -> Result<TcpReady> {
    let capabilities = capture.prepare_station(context, timeout)?;
    let direction_supported = match direction {
        Direction::Rx => capabilities.features.rx,
        Direction::Tx => capabilities.features.tx,
        Direction::Bidirectional => capabilities.features.bidirectional,
    };
    if !capabilities.features.tcp || !direction_supported {
        return Err(format!("firmware does not advertise TCP {direction:?} capability").into());
    }
    if !capabilities.features.runtime_configuration || !capabilities.features.structured_evidence {
        return Err("TCP RX requires runtime sessions and structured evidence".into());
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        capture.check_link()?;
        let address = capture.observed_protocol_ipv4(WifiNetworkInterface::Station);
        if capture.observed_service(
            WifiNetworkInterface::Station,
            Transport::Tcp,
            direction,
            port,
        ) && let Some(address) = address
        {
            return Ok(TcpReady { address });
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "device {address_hint}:{port} did not publish TCP {direction:?} readiness within {} seconds",
        timeout.as_secs(),
    )
    .into())
}

/// Provisions the station and returns only the typed `NetworkReady` address.
pub(crate) fn await_network_ready(
    capture: &SerialCapture,
    context: &Context<'_>,
    timeout: Duration,
) -> Result<Ipv4Addr> {
    capture.prepare_station(context, timeout)?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        capture.check_link()?;
        if let Some(address) = capture.observed_protocol_ipv4(WifiNetworkInterface::Station) {
            return Ok(address);
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "device did not publish typed network readiness within {} seconds",
        timeout.as_secs()
    )
    .into())
}
/// Wait until the target owns its IPv4 address and UDP RX service.
///
/// The qualification image requires a typed service-ready edge emitted only
/// after the target consumes a negative warm-up datagram. `Arm`/`Start` then
/// provide measured synchronization.
pub(crate) fn await_udp_rx_ready(
    capture: &SerialCapture,
    context: &Context<'_>,
    address_hint: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Result<UdpRxReady> {
    let capabilities = capture.prepare_station(context, timeout)?;
    if !capabilities.features.udp || !capabilities.features.rx {
        return Err("firmware does not advertise UDP RX capability".into());
    }
    if !capabilities.features.runtime_configuration || !capabilities.features.structured_evidence {
        return Err(
            "qualification firmware requires runtime sessions and structured evidence".into(),
        );
    }
    probe_udp_rx_ready(capture, address_hint, port, timeout)
}

/// Prove the already-running UDP RX service from its current network peer.
///
/// Unlike [`await_udp_rx_ready`], this does not prepare or assume the station
/// role. AP qualification calls it only after the controlled client has joined,
/// so the unmeasured datagram also establishes the AP-side neighbor/data path
/// before sequence zero is admitted to a measured session.
pub(crate) fn probe_udp_rx_ready(
    capture: &SerialCapture,
    address_hint: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Result<UdpRxReady> {
    probe_udp_rx_ready_via(
        capture,
        WifiNetworkInterface::Station,
        address_hint,
        None,
        port,
        timeout,
    )
}

/// Prove UDP RX readiness while an explicit routed peer owns the host path.
///
/// `address_hint` remains the target identity published in typed evidence;
/// `traffic_address` is only the address through which the probe reaches it.
/// This distinction is required when a controlled OpenWrt Wi-Fi station
/// forwards traffic from the wired HIL generator to the DUT AP.
pub(crate) fn probe_udp_rx_ready_via(
    capture: &SerialCapture,
    network_interface: WifiNetworkInterface,
    address_hint: Ipv4Addr,
    traffic_address: Option<Ipv4Addr>,
    port: u16,
    timeout: Duration,
) -> Result<UdpRxReady> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    let mut address = address_hint;
    let mut connected_address = traffic_address.unwrap_or(address);
    if !connected_address.is_unspecified() {
        socket.connect(SocketAddrV4::new(connected_address, port))?;
    }
    socket.set_write_timeout(Some(Duration::from_millis(250)))?;
    let mut packet = [0x5a; RX_PROBE_PAYLOAD];
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        capture.check_link()?;
        if let Some(discovered) = capture.observed_protocol_ipv4(network_interface)
            && discovered != address
        {
            address = discovered;
            if traffic_address.is_none() {
                connected_address = address;
                socket.connect(SocketAddrV4::new(connected_address, port))?;
            }
        }
        let rx_service_ready = capture.observed_udp_service(network_interface, Direction::Rx, port);
        let tx_service_ready =
            capture.observed_udp_service(network_interface, Direction::Tx, 4_324);
        if !rx_service_ready
            || !tx_service_ready
            || capture.observed_protocol_ipv4(network_interface).is_none()
        {
            thread::sleep(Duration::from_millis(20));
            continue;
        }

        let boot_id = capture
            .latest_boot_id()
            .ok_or("device did not publish a current HIL boot identity")?;
        let event_start = capture.protocol_event_count();
        packet[..4].copy_from_slice(&(-1_i32).to_be_bytes());
        socket.send(&packet)?;
        if capture
            .wait_for_protocol_after(event_start, RX_PROBE_RESPONSE_TIMEOUT, |message| {
                message.boot_id == boot_id
                    && matches!(
                        message.body,
                        Event::ServiceReady(service)
                            if service.transport == Transport::Udp
                                && service.direction == Direction::Rx
                                && service.local_port == port
                    )
            })?
            .is_some()
        {
            return Ok(UdpRxReady { address });
        }
    }

    Err(format!(
        "device {address}:{port} did not confirm end-to-end UDP RX within {} seconds",
        timeout.as_secs(),
    )
    .into())
}

pub(crate) fn await_udp_tx_ready(
    capture: &SerialCapture,
    context: &Context<'_>,
    address_hint: Ipv4Addr,
    timeout: Duration,
) -> Result<UdpTxReady> {
    let capabilities = capture.prepare_station(context, timeout)?;
    if !capabilities.features.udp || !capabilities.features.tx {
        return Err("firmware does not advertise UDP TX capability".into());
    }
    if !capabilities.features.runtime_configuration || !capabilities.features.structured_evidence {
        return Err(
            "qualification firmware requires runtime sessions and structured evidence".into(),
        );
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        capture.check_link()?;
        if capture.observed_udp_service(WifiNetworkInterface::Station, Direction::Tx, 4_324) {
            let discovery_deadline = Instant::now() + DHCP_DISCOVERY_GRACE;
            while Instant::now() < discovery_deadline {
                if let Some(address) = capture.observed_protocol_ipv4(WifiNetworkInterface::Station)
                {
                    return Ok(UdpTxReady { address });
                }
                thread::sleep(Duration::from_millis(10));
            }
            return Ok(UdpTxReady {
                address: address_hint,
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "device did not publish typed UDP TX readiness within {} seconds",
        timeout.as_secs(),
    )
    .into())
}
