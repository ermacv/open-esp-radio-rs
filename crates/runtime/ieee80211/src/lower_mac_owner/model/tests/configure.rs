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
                if matches!(event, LowerMacEvent::Configured(VifRole::Station)) {
                    configured += 1;
                }
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
                    Poll::Ready(Ok(Ok(LowerMacEvent::Configured(VifRole::Station))))
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
                if matches!(event, LowerMacEvent::Configured(_)) {
                    extra += 1;
                }
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
fn review_configured_drained_by_disable_is_rearmed() {
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
