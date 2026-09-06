//! Real released Embassy plus the production adapter and HIL observation boundary.
extern crate embassy_net_compat as embassy_net;
use embassy_net::driver::{Driver, RxToken, TxToken};
use embassy_net::tcp::TcpSocket;
use embassy_net::{Stack, udp::UdpSocket};
use open_esp_radio_embassy_net_compat::{FrameStorage, LinkState, NoopRawMutex, Resources};
use std::{
    future::Future,
    task::{Context, Waker},
};

#[path = "../../../targets/esp32s31/runtime/src/product_hil/network/embassy/ipv4.rs"]
mod ipv4;
#[path = "../../../targets/esp32s31/runtime/src/product_hil/network/progress.rs"]
mod progress;
#[path = "../../../targets/esp32s31/runtime/src/product_hil/network/progress/smoltcp.rs"]
mod progress_adapter;
#[path = "../../../targets/esp32s31/runtime/src/product_hil/network/sockets/smoltcp.rs"]
mod sockets;

type Device = open_esp_radio_embassy_net_compat::Device<'static, NoopRawMutex, 1536, 2>;
type Radio = open_esp_radio_embassy_net_compat::RadioRunner<'static, NoopRawMutex, 1536, 2>;
fn pair() -> (Device, Radio) {
    Box::leak(Box::new(Resources::new())).split(
        [2, 0, 0, 0, 0, 1],
        Box::leak(Box::new(FrameStorage::new())),
        Box::leak(Box::new(FrameStorage::new())),
    )
}

#[test]
fn tokens_are_counted_only_on_consumption_and_ownership_returns() {
    let (device, radio) = pair();
    radio.set_link_state(LinkState::Up);
    let counters = Box::leak(Box::new(progress::Counters::new()));
    let mut device = progress::Device::new(device, counters);
    let mut cx = Context::from_waker(Waker::noop());
    drop(device.transmit(&mut cx).unwrap());
    assert_eq!(radio.tx_queue_len(), 0);
    assert_eq!(counters.snapshot().get(progress::Event::TxAccepted), 0);
    device
        .transmit(&mut cx)
        .unwrap()
        .consume(60, |bytes| bytes.fill(42));
    let frame = radio.try_receive_tx().unwrap();
    assert_eq!(frame.as_slice(), &[42; 60]);
    drop(frame);
    assert_eq!(counters.snapshot().get(progress::Event::TxAccepted), 1);
    radio.try_send_rx(&[17; 60]).unwrap();
    let (rx, tx) = device.receive(&mut cx).unwrap();
    assert_eq!(counters.snapshot().get(progress::Event::RxDelivered), 0);
    rx.consume(|bytes| assert_eq!(bytes, &[17; 60]));
    drop(tx);
    assert_eq!(counters.snapshot().get(progress::Event::RxDelivered), 1);
    assert_eq!(counters.snapshot().get(progress::Event::TxAccepted), 1);
    let mut observed = std::pin::pin!(progress::observe(std::future::ready(7), counters));
    assert_eq!(observed.as_mut().poll(&mut cx), std::task::Poll::Ready(7));
    assert_eq!(
        counters
            .snapshot()
            .get(progress::Event::PollWithoutTransfer),
        1
    );
    let sample = counters.snapshot();
    assert_eq!(sample.delta(sample).get(progress::Event::TxRejected), 0);
}

#[test]
fn role_configurations_are_independent_and_udp_uses_released_socket_storage() {
    use open_esp_radio_hil_protocol::{
        NetworkIpv4Configuration as Config, WifiNetworkInterface as Role,
    };
    let (sta, _) = pair();
    let (ap, radio) = pair();
    radio.set_link_state(LinkState::Up);
    let mut sta_resources = embassy_net::StackResources::<4>::new();
    let mut ap_resources = embassy_net::StackResources::<4>::new();
    let (sta, _) = embassy_net::new(sta, Default::default(), &mut sta_resources, 1);
    let (ap, mut runner) = embassy_net::new(ap, Default::default(), &mut ap_resources, 2);
    let sta = ipv4::Iface(sta);
    let ap = ipv4::Iface(ap);
    for (iface, address) in [(sta, [192, 0, 2, 1]), (ap, [198, 51, 100, 1])] {
        ipv4::configure(
            iface,
            Some(Config::Static {
                address,
                prefix_length: 24,
                gateway: None,
            }),
        );
    }
    ipv4::configure(sta, None);
    assert!(ipv4::info(sta, Role::Station).is_none());
    assert_eq!(
        ipv4::info(ap, Role::AccessPoint).unwrap().address,
        [198, 51, 100, 1]
    );
    let mut storage = sockets::UdpStorage::<{ sockets::UDP_RX_QUEUE_DEPTH }, 16>::new();
    let mut socket = sockets::new_udp(ap.stack(), &mut storage);
    socket.bind(4321).unwrap();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut send = std::pin::pin!(socket.send_to(
            b"shared workload",
            (embassy_net::Ipv4Address::new(198, 51, 100, 255), 4321)
        ));
        assert!(matches!(
            send.as_mut().poll(&mut cx),
            std::task::Poll::Ready(Ok(()))
        ));
    }
    let mut network = std::pin::pin!(runner.run());
    assert!(network.as_mut().poll(&mut cx).is_pending());
    let frame = radio
        .try_receive_tx()
        .expect("stack publishes queued datagram to the driver");
    radio.try_send_rx(frame.as_slice()).unwrap();
    drop(frame);
    assert!(network.as_mut().poll(&mut cx).is_pending());
    let receive = sockets::recv_from_with(&mut socket, |bytes, _| bytes.to_vec());
    assert_eq!(
        std::pin::pin!(receive).as_mut().poll(&mut cx),
        std::task::Poll::Ready(Ok(b"shared workload".to_vec()))
    );
    let mut port = sockets::listen(ap.stack(), 4322);
    let mut rx = [0; 1024];
    let mut tx = [0; 1024];
    let mut tcp = TcpSocket::new(ap.stack(), &mut rx, &mut tx);
    assert!(
        std::pin::pin!(sockets::accept(&mut port, &mut tcp))
            .as_mut()
            .poll(&mut cx)
            .is_pending()
    );
    assert!(
        std::pin::pin!(ap.wait_config_v4_up())
            .as_mut()
            .poll(&mut cx)
            .is_ready()
    );
    assert!(
        std::pin::pin!(ap.wait_config_v4_down())
            .as_mut()
            .poll(&mut cx)
            .is_pending()
    );
    assert!(
        std::pin::pin!(sta.wait_config_v4_down())
            .as_mut()
            .poll(&mut cx)
            .is_ready()
    );
}
