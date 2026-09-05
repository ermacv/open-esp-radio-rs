use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_esp32s31_coex::{
    CoexClient, CoexClockSelector, CoexPti, CoexPtiTable, CoexTimerClock, CoexTimerIndex,
};

use super::*;

#[derive(Default)]
struct Hardware {
    enabled: u8,
    disabled: u8,
}

impl CoexTimerHardware for Hardware {
    fn configure_request(
        &mut self,
        _index: CoexTimerIndex,
        _client: CoexClient,
        _pti: CoexPti,
    ) -> Result<(), CoexError> {
        Ok(())
    }

    fn set_primary_target(
        &mut self,
        _index: CoexTimerIndex,
        _tick_image: u32,
    ) -> Result<(), CoexError> {
        Ok(())
    }

    fn set_secondary_target(
        &mut self,
        _index: CoexTimerIndex,
        _tick_image: u32,
    ) -> Result<(), CoexError> {
        Ok(())
    }

    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.enabled |= 1 << index.value();
        Ok(())
    }

    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.disabled |= 1 << index.value();
        self.enabled &= !(1 << index.value());
        Ok(())
    }

    fn force(&mut self, _index: CoexTimerIndex) -> Result<(), CoexError> {
        Ok(())
    }

    fn unforce(&mut self, _index: CoexTimerIndex) -> Result<(), CoexError> {
        Ok(())
    }
}

struct Clock(CoexTimerClock);

impl CoexClockHardware for Clock {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
        Ok(self.0)
    }
}

#[test]
fn single_owner_serializes_request_release_and_shutdown() {
    let mut resources = CoexResources::<NoopRawMutex, 2>::new();
    let (mut control, owner) = resources.split();
    let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
    let mut hardware = Hardware::default();
    let mut clock = Clock(CoexTimerClock::from_hardware_fields(
        CoexClockSelector::Selector8,
        0,
        40,
        true,
    ));
    let request = CoexClientRequest {
        event: CoexEventId::new(1).unwrap(),
        latency: 1_000,
        duration: 2_000,
    };

    block_on(join(
        async {
            assert_eq!(
                control.execute(CoexCommand::Enable).await,
                Ok(CoexOutcome::Status(CoexStatus {
                    enabled: true,
                    active_timers: 0,
                }))
            );
            assert_eq!(
                control.execute(CoexCommand::WifiRequest(request)).await,
                Ok(CoexOutcome::Request(CoexTimerIndex::new(0).unwrap()))
            );
            assert_eq!(
                control.execute(CoexCommand::Release(request.event)).await,
                Ok(CoexOutcome::Release(CoexTimerIndex::new(0).unwrap()))
            );
            assert_eq!(
                control.execute(CoexCommand::Shutdown).await,
                Ok(CoexOutcome::Stopped)
            );
        },
        owner.run(&mut core, &mut hardware, &mut clock),
    ))
    .1
    .unwrap();

    assert_eq!(hardware.enabled, 0);
    assert_eq!(hardware.disabled, 1);
}
