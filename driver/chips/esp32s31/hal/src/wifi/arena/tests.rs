use crate::{MacInterruptSetup, RadioHardware, RadioRuntimeOwner};

use super::*;

#[test]
fn stable_publication_reclaims_exactly_once_and_drop_poison_is_sticky() {
    let cold = RadioHardware::for_validation().into_wifi();
    let (registers, _interrupt_setup) = cold.into_running();
    let owner = RadioRuntimeOwner::from_pac(registers);
    let arena = Esp32s31RadioOwnerArena::new();
    let published = arena
        .publish(owner)
        .unwrap_or_else(|_| panic!("an empty arena must accept its first owner"));
    assert_eq!(arena.state(), Esp32s31RadioOwnerArenaState::Published);

    let borrowed = arena.registers.borrow();
    let published = match published.try_reclaim() {
        Ok(_) => panic!("an outstanding transaction must prevent reclaim"),
        Err((published, error)) => {
            assert_eq!(error, Esp32s31RadioOwnerArenaError::Borrowed);
            published
        }
    };
    drop(borrowed);
    let reclaimed = published
        .try_reclaim_with_republish()
        .unwrap_or_else(|_| panic!("a returned transaction must permit exact reclaim"));
    assert_eq!(arena.state(), Esp32s31RadioOwnerArenaState::Empty);

    let poisoned = Esp32s31RadioOwnerArena::new();
    let published = poisoned
        .publish(reclaimed.into_owner())
        .unwrap_or_else(|_| panic!("the second empty arena must accept the reclaimed owner"));
    drop(published);
    assert_eq!(
        poisoned.state(),
        Esp32s31RadioOwnerArenaState::ResetRequired
    );
}

#[test]
fn reclaimed_owner_republishes_only_through_its_exact_arena_binding() {
    let cold = RadioHardware::for_validation().into_wifi();
    let (registers, _interrupt_setup) = cold.into_running();
    let owner = RadioRuntimeOwner::from_pac(registers);
    let arena = Esp32s31RadioOwnerArena::new();
    let published = arena
        .publish(owner)
        .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
    let reclaimed = published
        .try_reclaim_with_republish()
        .unwrap_or_else(|_| panic!("a quiescent lease must retain its arena binding"));
    assert_eq!(arena.state(), Esp32s31RadioOwnerArenaState::Empty);

    let published = reclaimed
        .try_republish()
        .unwrap_or_else(|_| panic!("the exact empty arena must accept republication"));
    assert_eq!(arena.state(), Esp32s31RadioOwnerArenaState::Published);
    let _registers = published
        .try_reclaim()
        .unwrap_or_else(|_| panic!("the republished owner must remain reclaimable"));
}

#[test]
fn published_channel_capability_holds_the_arena_serialization_guard() {
    let cold = RadioHardware::for_validation().into_wifi();
    let (registers, _interrupt_setup) = cold.into_running();
    let owner = RadioRuntimeOwner::from_pac(registers);
    let arena = Esp32s31RadioOwnerArena::new();
    let published = arena
        .publish(owner)
        .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
    let access = published.access();
    let mut platform = ();
    let channel = access
        .try_channel_hal(&mut platform)
        .unwrap_or_else(|_| panic!("published registers must yield a channel capability"));

    let published = match published.try_reclaim() {
        Ok(_) => panic!("a live channel capability must prevent reclaim"),
        Err((published, error)) => {
            assert_eq!(error, Esp32s31RadioOwnerArenaError::Borrowed);
            published
        }
    };
    drop(channel);
    let _registers = published
        .try_reclaim()
        .unwrap_or_else(|_| panic!("dropping the channel capability must release the guard"));
}

#[test]
fn published_wifi_mac_capability_holds_the_arena_serialization_guard() {
    let cold = RadioHardware::for_validation().into_wifi();
    let (registers, _interrupt_setup) = cold.into_running();
    let owner = RadioRuntimeOwner::from_pac(registers);
    let arena = Esp32s31RadioOwnerArena::new();
    let published = arena
        .publish(owner)
        .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
    let access = published.access();
    let wifi_mac = access
        .try_wifi_mac_hal()
        .unwrap_or_else(|_| panic!("published registers must yield a Wi-Fi MAC capability"));

    let published = match published.try_reclaim() {
        Ok(_) => panic!("a live Wi-Fi MAC capability must prevent reclaim"),
        Err((published, error)) => {
            assert_eq!(error, Esp32s31RadioOwnerArenaError::Borrowed);
            published
        }
    };
    drop(wifi_mac);
    let _registers = published
        .try_reclaim()
        .unwrap_or_else(|_| panic!("dropping the Wi-Fi MAC capability must release the guard"));
}

#[test]
fn stale_access_cannot_mutate_a_reset_required_arena() {
    let cold = RadioHardware::for_validation().into_wifi();
    let (registers, interrupt_setup) = cold.into_running();
    let owner = RadioRuntimeOwner::from_pac(registers);
    let mut interrupt_setup = MacInterruptSetup {
        inner: interrupt_setup,
    };
    let arena = Esp32s31RadioOwnerArena::new();
    let published = arena
        .publish(owner)
        .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
    let access = published.access();

    drop(published);
    assert_eq!(arena.state(), Esp32s31RadioOwnerArenaState::ResetRequired);
    assert!(matches!(
        access.try_prepare_connected_sta_without_power_save(&mut interrupt_setup),
        Err(Esp32s31RadioOwnerArenaError::ResetRequired)
    ));
}
