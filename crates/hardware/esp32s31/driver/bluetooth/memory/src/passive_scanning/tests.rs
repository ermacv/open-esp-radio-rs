use std::boxed::Box;

use oer_esp32s31_hal::bluetooth::BluetoothControllerLatchedTime;

use super::{
    LINK_STATE_RX_CLASS_WORD, PassiveScanError, PassiveScanPool, PassiveScanStorage,
    SCHEDULER_ITEM_ALLOCATION_NUMBER_WORD,
};
use crate::{
    PassiveScanDefaultTxPowerDbm, PassiveScanPrimaryChannel, PassiveScanResetConfig,
    PassiveScanSchedulerWindow, PassiveScanStartSelection,
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_rx_chain::{LeRxChain, LeRxChainModelAddress, LeRxChainStorage},
    passive_scanning_event_image::PassiveScanRxHeadProjection,
    rx_memory_list::RxMemoryListClass,
    scheduler_pool::{
        SchedulerAllocationConfig, SchedulerPoolError, SchedulerPoolModelAddress,
        SchedulerRoleInstance, SchedulerRolePoolStorage,
    },
};

fn pool() -> PassiveScanPool<1> {
    let storage = Box::leak(Box::new(
        SchedulerRolePoolStorage::<PassiveScanStorage, 1>::new(),
    ));
    let numbers = SchedulerAllocationConfig::new(2, 3, 0).unwrap().scanning();
    PassiveScanPool::bind_model(
        storage,
        SchedulerPoolModelAddress::new(0x2f00_0100).expect("test base is aligned"),
        numbers,
    )
    .expect("the pool fits controller SRAM")
}

fn chain(class: RxMemoryListClass) -> LeRxChain<2> {
    let storage = Box::leak(Box::new(LeRxChainStorage::<2>::new()));
    LeRxChain::bind_model(
        storage,
        class,
        LeRxChainModelAddress::new(0x2f00_8000).expect("test base is aligned"),
    )
    .unwrap()
}

fn config() -> PassiveScanResetConfig {
    PassiveScanResetConfig::le_1m_public_accept_all(
        PassiveScanDefaultTxPowerDbm::new(0),
        BluetoothControllerLatchedTime::from_bits(0x1234_5678),
    )
}

fn window() -> PassiveScanSchedulerWindow {
    PassiveScanSchedulerWindow::from_controller_ticks(1_000, 2_000).unwrap()
}

fn reset(pool: &mut PassiveScanPool<1>) -> SchedulerRoleInstance {
    let instance = pool.acquire().unwrap();
    pool.reset(&instance, &chain(RxMemoryListClass::Scanning), config())
        .unwrap();
    instance
}

fn prepare(
    pool: &mut PassiveScanPool<1>,
    instance: &SchedulerRoleInstance,
) -> super::PassiveScanEvent {
    pool.prepare_event(
        instance,
        PassiveScanPrimaryChannel::Channel38,
        window(),
        PassiveScanStartSelection::Requested,
        BluetoothControllerLatchedTime::from_bits(0x5555),
    )
    .unwrap()
}

#[test]
fn the_reset_joins_the_scanning_chain() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    let chain = chain(RxMemoryListClass::Scanning);
    pool.reset(&instance, &chain, config()).unwrap();
    let (graph, binding, _) = pool.shared(&instance).unwrap();
    let image = graph.link_state.image();
    assert!(image.retains_rx_head(PassiveScanRxHeadProjection::from_bound(chain.head_link())));
    assert_eq!(image.crc_init(), LeCrcInit::LE_PRESET);
    assert_eq!(image.access_address(), LeAccessAddress::PRIMARY_ADVERTISING);
    assert_eq!(image.controller_time(), 0x1234_5678);
    assert_eq!(
        graph.link_state.words[LINK_STATE_RX_CLASS_WORD].get() >> 28,
        1
    );
    // The free chain survives the reset.
    assert_eq!(
        graph.link_state.free_head(),
        binding.items[2].controller_address().address()
    );
}

#[test]
fn items_are_numbered_after_the_connections() {
    let pool = pool();
    let mut pool = pool;
    let instance = pool.acquire().unwrap();
    let (graph, _, _) = pool.shared(&instance).unwrap();
    for (index, item) in graph.items.iter().enumerate() {
        assert_eq!(
            item.words[SCHEDULER_ITEM_ALLOCATION_NUMBER_WORD].get(),
            2 + 1 + 3 + index as u32
        );
    }
}

#[test]
fn a_window_takes_the_free_head_and_finishing_returns_it() {
    let mut pool = pool();
    let instance = reset(&mut pool);
    let event = prepare(&mut pool, &instance);
    assert_eq!(event.item(), 2);
    assert_eq!(event.raw_window(), (1_000, 2_000));
    {
        let (graph, binding, _) = pool.shared(&instance).unwrap();
        assert_eq!(graph.items[2].header().hardware_next_image(), 0);
        assert_eq!(
            graph.link_state.free_head(),
            binding.items[1].controller_address().address()
        );
        assert_eq!(graph.link_state.image().controller_time(), 0x5555);
        assert_eq!(
            (
                graph.items[2].header().raw_start(),
                graph.items[2].header().raw_end()
            ),
            (1_000, 2_000)
        );
    }
    assert_eq!(
        pool.submit(&instance, 1),
        Err(SchedulerPoolError::NotPrepared)
    );
    let id = pool.submit(&instance, 2).unwrap();
    assert_eq!(
        pool.finish_event(&instance),
        Err(PassiveScanError::Pool(SchedulerPoolError::InstanceBusy))
    );
    pool.retire(id).unwrap();
    pool.finish_event(&instance).unwrap();

    let (graph, binding, _) = pool.shared(&instance).unwrap();
    assert_eq!(
        graph.link_state.free_head(),
        binding.items[2].controller_address().address()
    );
    assert_eq!(
        graph.items[2].header().hardware_next_image(),
        binding.items[1].compressed_image()
    );
}

#[test]
fn each_item_receives_under_its_own_number() {
    let mut pool = pool();
    let instance = reset(&mut pool);
    let first = pool.receive_source(&instance, 0).unwrap();
    let last = pool.receive_source(&instance, 2).unwrap();
    assert_ne!(first, last);
    assert_eq!(last.class(), RxMemoryListClass::Scanning);
}

#[test]
fn a_non_scanning_chain_is_refused() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    assert_eq!(
        pool.reset(&instance, &chain(RxMemoryListClass::NonScanning), config()),
        Err(PassiveScanError::ForeignReceiveClass)
    );
    assert_eq!(
        pool.prepare_event(
            &instance,
            PassiveScanPrimaryChannel::Channel37,
            window(),
            PassiveScanStartSelection::Requested,
            BluetoothControllerLatchedTime::from_bits(0),
        ),
        Err(PassiveScanError::State)
    );
}
