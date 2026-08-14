use core::{
    future::{Future, poll_fn},
    pin::pin,
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
};

use embassy_net::{Config, Ipv4Address, Ipv4Cidr, StackResources, StaticConfigV4};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_embassy_net::{PinnedTxPool, SplitPinnedResources};

const FRAME_CAPACITY: usize = 1_536;
const TX_HEADROOM: usize = 28;
const TX_TRAILER: usize = 8;

#[test]
fn role_change_delivers_arp_to_the_reconfigured_embassy_stack() {
    let done = Box::leak(Box::new(AtomicBool::new(false)));
    let executor = Box::leak(Box::new(embassy_executor::Executor::new()));
    executor.run_until(
        |spawner| {
            spawner.spawn(run_role_change_arp_test(done).expect("test task allocates once"));
        },
        || done.load(Ordering::Acquire),
    );
}

#[embassy_executor::task]
async fn run_role_change_arp_test(done: &'static AtomicBool) {
    type Resources =
        SplitPinnedResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2, 2>;
    type TxPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;

    let resources = Box::leak(Box::new(Resources::new()));
    let tx_pool = TxPool::pin_static(Box::leak(Box::new(TxPool::new())));
    let station = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let access_point = [0x32, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let client = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];
    let (device, radio) = resources.split(tx_pool, station);
    let stack_resources = Box::leak(Box::new(StackResources::<1>::new()));
    let (stack, mut runner) = embassy_net::new(
        device,
        Config::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::new(10, 43, 0, 1), 24),
            gateway: None,
            dns_servers: Default::default(),
        }),
        stack_resources,
        1,
    );

    radio.set_hardware_address(access_point);
    radio.set_link_state(embassy_net_driver::LinkState::Up);
    radio.try_send_rx(&arp_request(client)).unwrap();

    let mut run = pin!(runner.run());
    let reply = poll_fn(|context| {
        assert!(run.as_mut().poll(context).is_pending());
        match radio.try_receive_tx() {
            Some(reply) => Poll::Ready(reply),
            None => {
                context.waker().wake_by_ref();
                Poll::Pending
            }
        }
    })
    .await;
    let bytes = reply.ethernet();
    assert_eq!(&bytes[..6], &client);
    assert_eq!(&bytes[6..12], &access_point);
    assert_eq!(&bytes[12..14], &0x0806_u16.to_be_bytes());
    assert_eq!(&bytes[20..22], &2_u16.to_be_bytes());
    assert_eq!(&bytes[22..28], &access_point);
    assert_eq!(&bytes[28..32], &[10, 43, 0, 1]);
    assert_eq!(
        stack.config_v4().unwrap().address.address(),
        Ipv4Address::new(10, 43, 0, 1)
    );
    done.store(true, Ordering::Release);
}

fn arp_request(client: [u8; 6]) -> [u8; 42] {
    let mut frame = [0_u8; 42];
    frame[..6].fill(0xff);
    frame[6..12].copy_from_slice(&client);
    frame[12..14].copy_from_slice(&0x0806_u16.to_be_bytes());
    frame[14..16].copy_from_slice(&1_u16.to_be_bytes());
    frame[16..18].copy_from_slice(&0x0800_u16.to_be_bytes());
    frame[18] = 6;
    frame[19] = 4;
    frame[20..22].copy_from_slice(&1_u16.to_be_bytes());
    frame[22..28].copy_from_slice(&client);
    frame[28..32].copy_from_slice(&[10, 43, 0, 2]);
    frame[38..42].copy_from_slice(&[10, 43, 0, 1]);
    frame
}
