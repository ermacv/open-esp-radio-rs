use super::*;

struct ConfigureAcrossDisable {
    posted_before_disable: bool,
}
impl PortBody<ModelBackend> for ConfigureAcrossDisable {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        _resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend> {
        let Ok(mut enabled) = owner.enable().await else {
            panic!()
        };
        enabled.phase().station().configure(StationReceive::Off);
        enabled.phase().station().configure(StationReceive::Off);
        if self.posted_before_disable {
            for _ in 0..3 {
                yield_now().await;
            }
        }
        let mut configured = 0;
        let Ok(disabled) = enabled
            .disable(&mut |event| {
                if matches!(event, LowerMacTerminal::Configured(VifRole::Station)) {
                    configured += 1;
                }
                core::ops::ControlFlow::Continue(())
            })
            .await
        else {
            panic!()
        };
        assert_eq!(
            configured, 1,
            "coalesced configuration drained exactly once"
        );
        let Ok(mut enabled) = disabled.enable().await else {
            panic!()
        };
        for _ in 0..2 {
            enabled.phase().station().configure(StationReceive::Off);
            enabled.phase().station().configure(StationReceive::Off);
            for _ in 0..3 {
                yield_now().await;
            }
            let mut phase = enabled.phase();
            let mut cx = core::task::Context::from_waker(Waker::noop());
            assert!(
                matches!(
                    core::pin::pin!(phase.next_event()).poll(&mut cx),
                    Poll::Ready(Ok(Ok(LowerMacEvent::Terminal(
                        LowerMacTerminal::Configured(VifRole::Station)
                    ))))
                ),
                "Configured suppressed after a previous extraction"
            );
            assert!(
                core::pin::pin!(phase.next_event())
                    .poll(&mut cx)
                    .is_pending(),
                "duplicate Configured"
            );
        }
        let mut extra = 0;
        let Ok(disabled) = enabled
            .disable(&mut |event| {
                if matches!(event, LowerMacTerminal::Configured(_)) {
                    extra += 1;
                }
                core::ops::ControlFlow::Continue(())
            })
            .await
        else {
            panic!()
        };
        assert_eq!(extra, 0);
        disabled.uninstall(())
    }
}

#[test]
fn configured_drained_by_disable_is_rearmed() {
    for posted_before_disable in [false, true] {
        let (backend, _) = backend();
        block_on(run_port(
            backend,
            ConfigureAcrossDisable {
                posted_before_disable,
            },
        ));
    }
}

struct ReceiveAcrossDisable;
impl PortBody<ModelBackend> for ReceiveAcrossDisable {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        _resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend> {
        let Ok(mut enabled) = owner.enable().await else {
            panic!()
        };
        let air = enabled.phase().held.air.clone();
        let meta = ModelRxMeta::new(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(), -40, -95);
        for _ in 0..MODEL_RX_EVENTS + 1 {
            air.receive(&[1], meta);
        }
        for _ in 0..3 {
            yield_now().await;
        }
        let mut frames = Vec::new();
        let event = enabled.phase().next_event().await.unwrap().unwrap();
        let LowerMacEvent::Received { frame, .. } = event else {
            panic!()
        };
        frames.push(frame);
        let Ok(disabled) = enabled
            .disable(&mut |_| panic!("RX cannot reach the terminal sink"))
            .await
        else {
            panic!()
        };
        assert_eq!(frames.len(), 1);
        let Ok(mut enabled) = disabled.enable().await else {
            panic!()
        };
        assert_eq!(
            enabled.phase().held.rx_out.get(),
            1,
            "only the buffer already lent to the body stays charged"
        );
        assert!(
            matches!(enabled.phase().next_event().await, Ok(Err(EventsLost))),
            "an unobserved loss survives disable"
        );
        air.receive(&[2], meta);
        let LowerMacEvent::Received { frame, .. } =
            enabled.phase().next_event().await.unwrap().unwrap()
        else {
            panic!("disable recycled unread RX capacity")
        };
        assert_eq!(frame.bytes(), &[2]);
        assert_eq!(enabled.phase().held.rx_out.get(), 2);
        enabled.phase().recycle(frame);
        for _ in 0..3 {
            yield_now().await;
        }
        assert_eq!(
            enabled.phase().held.rx_out.get(),
            1,
            "the pre-disable owner is still charged"
        );
        for frame in frames {
            enabled.phase().recycle(frame);
        }
        for _ in 0..3 {
            yield_now().await;
        }
        assert_eq!(enabled.phase().held.rx_out.get(), 0);
        air.receive(&[3], meta);
        let event = enabled.phase().next_event().await.unwrap().unwrap();
        let LowerMacEvent::Received { frame, .. } = event else {
            panic!()
        };
        assert_eq!(frame.bytes(), [3]);
        enabled.phase().recycle(frame);
        let Ok(disabled) = enabled
            .disable(&mut |_| panic!("all events consumed"))
            .await
        else {
            panic!()
        };
        disabled.uninstall(())
    }
}

#[test]
fn disable_preserves_borrowed_rx_capacity_and_unobserved_loss() {
    let (backend, _) = backend();
    block_on(run_port(backend, ReceiveAcrossDisable));
}
