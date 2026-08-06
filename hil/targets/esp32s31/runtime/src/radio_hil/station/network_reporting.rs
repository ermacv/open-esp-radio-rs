#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_net::{
    Ipv4Address, Stack,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_time::Timer;
use open_esp_radio_hil_protocol::{Event as HilEvent, NetworkInfo};

use crate::console::emergency_log;

/// Explicit HIL-only inputs for DHCP/readiness reporting and the LAN ARP probe.
#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilNetworkReportBindings {
    local_ipv4: &'static AtomicU32,
    lan_probe_response: &'static AtomicBool,
    lan_probe_rx_s_mpdu: &'static AtomicU32,
    lan_probe_ipv4: [u8; 4],
}

impl RadioHilNetworkReportBindings {
    pub(in crate::radio_hil) const fn new(
        local_ipv4: &'static AtomicU32,
        lan_probe_response: &'static AtomicBool,
        lan_probe_rx_s_mpdu: &'static AtomicU32,
        lan_probe_ipv4: [u8; 4],
    ) -> Self {
        Self {
            local_ipv4,
            lan_probe_response,
            lan_probe_rx_s_mpdu,
            lan_probe_ipv4,
        }
    }
}

#[embassy_executor::task]
pub(in crate::radio_hil) async fn connected_network_report_task(
    stack: Stack<'static>,
    bindings: RadioHilNetworkReportBindings,
) {
    report_network_configuration(stack, bindings).await
}

async fn report_network_configuration(
    stack: Stack<'_>,
    bindings: RadioHilNetworkReportBindings,
) -> ! {
    for elapsed_ms in 0..15_000_u32 {
        if let Some(config) = stack.config_v4() {
            let local_ipv4 = config.address.address().octets();
            crate::console::publish_event(
                0,
                0,
                HilEvent::NetworkReady(NetworkInfo {
                    address: local_ipv4,
                    prefix_length: config.address.prefix_len(),
                    gateway: config.gateway.map(|address| address.octets()),
                }),
            );
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-dhcp \
                 address={} gateway={:?} dns={:?} elapsed_ms={elapsed_ms}",
                config.address, config.gateway, config.dns_servers,
            ));

            // Keep probe generation at the network/application boundary.
            // Sending one ordinary UDP datagram makes embassy-net resolve
            // the peer through ARP; the HIL RX observer records only that the
            // matching reply crossed the production driver.
            bindings
                .local_ipv4
                .store(u32::from_be_bytes(local_ipv4), Ordering::Release);
            let mut probe_rx_metadata = [PacketMetadata::EMPTY; 1];
            let mut probe_rx_buffer = [0_u8; 1];
            let mut probe_tx_metadata = [PacketMetadata::EMPTY; 1];
            let mut probe_tx_buffer = [0_u8; 1];
            let mut probe_socket = UdpSocket::new(
                stack,
                &mut probe_rx_metadata,
                &mut probe_rx_buffer,
                &mut probe_tx_metadata,
                &mut probe_tx_buffer,
            );
            if let Err(error) = probe_socket.bind(4_325) {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                     stage=embassy-net-external-probe error=bind-{error:?}"
                ));
            } else if let Err(error) = probe_socket
                .send_to(&[0], (Ipv4Address::from_octets(bindings.lan_probe_ipv4), 9))
                .await
            {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                     stage=embassy-net-external-probe error=send-{error:?}"
                ));
            }
            for _ in 0..5_000 {
                if bindings.lan_probe_response.load(Ordering::Acquire) {
                    Timer::after_millis(10).await;
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=PASS \
                         stage=embassy-net-external-probe-ready address={} rx_s_mpdu={}",
                        config.address,
                        bindings.lan_probe_rx_s_mpdu.load(Ordering::Relaxed),
                    ));
                    loop {
                        Timer::after_secs(60).await;
                    }
                }
                Timer::after_millis(1).await;
            }
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=embassy-net-external-probe-ready error=arp-prime-timeout \
                 address={}",
                config.address,
            ));
            loop {
                Timer::after_secs(60).await;
            }
        }
        Timer::after_millis(1).await;
    }

    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=FAIL stage=embassy-net-dhcp error=timeout"
    ));
    loop {
        Timer::after_secs(60).await;
    }
}
