use super::*;
use embassy_sync::signal::Signal;
use oer_radio_port::{Disabled, Ended, Owner, PortBody, PortEnd, run_port};

struct FailWhileActive<'a> {
    model: Model,
    stop: &'a Signal<NoopRawMutex, ()>,
    finish: &'a Signal<NoopRawMutex, ()>,
}

impl PortBody<Backend> for FailWhileActive<'_> {
    type Output = u32;

    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend, Disabled>,
        (): (),
    ) -> Ended<'id, Backend, u32> {
        use oer_bluetooth_radio::LeRadioBackend;
        let Ok(mut owner) = owner.enable().await else {
            panic!("enable")
        };
        {
            let mut phase = owner.phase();
            assert_eq!(
                Backend::submit(&mut phase.clients, configure()).await,
                Ok(Ok(()))
            );
            assert_eq!(
                Backend::submit(&mut phase.clients, advertise(1, 10_000)).await,
                Ok(Ok(()))
            );
        }
        self.stop.wait().await;
        assert!(
            self.model.0.borrow().running,
            "the stop interrupts active work"
        );
        let proof = owner.fail_stop();
        assert_eq!(
            matches!(proof.outcome(), oer_radio_port::Abandon::Unconfirmed(_)),
            self.model.0.borrow().stop_never_settles
        );
        assert!(
            self.model.0.borrow().quarantined,
            "quarantine is synchronous"
        );
        self.finish.wait().await;
        Ended::fail_stopped(proof, 73)
    }
}

fn drive_until_running(port: core::pin::Pin<&mut impl Future>, model: &Model) {
    let mut port = port;
    for _ in 0..128 {
        assert!(embassy_futures::poll_once(port.as_mut()).is_pending());
        if model.0.borrow().running {
            return;
        }
        clock()
            .advance(oer_time::Duration::from_micros(100))
            .unwrap();
    }
    panic!("the admitted event did not start")
}

#[test]
fn fail_stop_quarantines_active_hardware_before_the_body_returns_and_carries_output() {
    for confirmed in [true, false] {
        let model = Model::default();
        model.0.borrow_mut().defer_execution = true;
        model.0.borrow_mut().stop_never_settles = !confirmed;
        let runtime = new_runtime();
        let installed = block_on(runtime.runtime.install(
            model_memory(),
            model.clone(),
            ModelEmergency(model.clone()),
        ))
        .unwrap_or_else(|_| panic!("install"));
        let stop = Signal::new();
        let finish = Signal::new();
        let mut port = core::pin::pin!(run_port(
            installed,
            FailWhileActive {
                model: model.clone(),
                stop: &stop,
                finish: &finish
            }
        ));
        drive_until_running(port.as_mut(), &model);
        stop.signal(());
        assert!(embassy_futures::poll_once(port.as_mut()).is_pending());
        assert_eq!(model.0.borrow().emergency_stops, 1);
        assert_eq!(model.0.borrow().running, !confirmed);
        assert!(model.0.borrow().quarantined);
        // The body still waits, but no later task operation can reach hardware.
        let before = model.0.borrow().scan_starts;
        assert_eq!(model.clone().publish_scan_start(), Err(EmergencyClosed));
        assert_eq!(model.0.borrow().scan_starts, before);
        assert_eq!(
            block_on(runtime.runtime.read_now()),
            Err(Poisoned {
                cause: BluetoothFault::Runner(crate::BluetoothRuntimeFault::Abandoned)
            })
        );
        finish.signal(());
        assert!(matches!(
            block_on(port),
            PortEnd::FailStopped { output: 73 }
        ));
        assert_eq!(model.0.borrow().emergency_stops, 1);
        let Err((error, ..)) = block_on(runtime.runtime.install(
            model_memory(),
            model.clone(),
            ModelEmergency(model.clone()),
        )) else {
            panic!("quarantined runtime reinstalled")
        };
        assert_eq!(error, BluetoothInstallError::Abandoned);
    }
}

struct DropOwner;
impl PortBody<Backend> for DropOwner {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend, Disabled>,
        (): (),
    ) -> Ended<'id, Backend, ()> {
        drop(owner);
        core::future::pending().await
    }
}

#[test]
fn a_dropped_owner_stops_even_when_its_body_never_returns() {
    let model = Model::default();
    let runtime = new_runtime();
    let installed = block_on(runtime.runtime.install(
        model_memory(),
        model.clone(),
        ModelEmergency(model.clone()),
    ))
    .unwrap_or_else(|_| panic!("install"));
    assert!(matches!(
        block_on(run_port(installed, DropOwner)),
        PortEnd::Abandoned
    ));
    assert_eq!(model.0.borrow().emergency_stops, 1);
    assert!(model.0.borrow().quarantined);
}

#[test]
fn cancelling_the_whole_port_stops_the_driver_without_waiting_for_the_body() {
    let model = Model::default();
    model.0.borrow_mut().defer_execution = true;
    let runtime = new_runtime();
    let installed = block_on(runtime.runtime.install(
        model_memory(),
        model.clone(),
        ModelEmergency(model.clone()),
    ))
    .unwrap_or_else(|_| panic!("install"));
    let stop = Signal::new();
    let finish = Signal::new();
    let mut port = Box::pin(run_port(
        installed,
        FailWhileActive {
            model: model.clone(),
            stop: &stop,
            finish: &finish,
        },
    ));
    drive_until_running(port.as_mut(), &model);
    drop(port);
    assert_eq!(model.0.borrow().emergency_stops, 1);
    assert!(!model.0.borrow().running);
    assert!(model.0.borrow().quarantined);
}
