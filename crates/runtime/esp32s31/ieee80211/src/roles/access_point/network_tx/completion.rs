//! Deadline waits, hardware completion and aggregate terminal observations.
//! These methods borrow the same owner used for publication and power save.

use super::*;

impl<'observer, B, N> AccessPointNetworkTx<'observer, B, N>
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
{
    pub(in super::super) async fn wait_deadline<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if let Some(phase) = self.aggregate_phase {
            let deadline = phase.deadline();
            let (_, ordinary) = control
                .mac
                .try_aggregate_adapter()
                .expect("aggregate publication leaves ordinary AP TX idle");
            ordinary.wait_until(deadline).await;
        } else {
            control.wait_tx_deadline().await;
        }
    }

    pub(in super::super) fn service<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware
            + ApRuntimeHardware
            + RxBlockAckHardware
            + oer_esp32s31_wifi_mac::tx::ampdu::HtAmpduHardware,
    {
        if self.aggregate_phase.is_none() {
            #[cfg(any(feature = "diagnostics", test))]
            let data = control.mac.pending_publication_kind()
                == Some(oer_esp32s31_wifi_ap::transaction::ApPendingPublicationKind::Data);
            let progress = match control.service_tx(hardware, wake) {
                Ok(progress) => progress,
                Err(error) => {
                    if self.active_group_release.is_some() {
                        self.complete_active_group_release(control, false)?;
                    }
                    if self.active_buffered_release.is_some() {
                        self.complete_active_buffered_release(control, false)?;
                    }
                    return Err(AccessPointDatapathError::Control(error));
                }
            };
            if progress == WifiTxProgress::Complete {
                let airtime_result = if let Some(accounting) = self.airtime.as_mut() {
                    accounting.complete_active(control.mac.work())
                } else {
                    Ok(())
                };
                #[cfg(any(feature = "diagnostics", test))]
                if data && let Some(observer) = self.observer {
                    observer.observe(AggregateTxObservation::OrdinaryWorkCompleted {
                        work: control.mac.work(),
                    });
                }
                let succeeded = control.take_last_terminal_tx_succeeded().unwrap_or(false);
                if self.active_group_release.is_some() {
                    // A group MPDU has no ACK. `succeeded` is only terminal
                    // hardware publication success for the one-attempt basic-
                    // rate transaction.
                    self.complete_active_group_release(control, succeeded)?;
                }
                if self.active_buffered_release.is_some() {
                    self.complete_active_buffered_release(control, succeeded)?;
                }
                let _ = self.stage_dtim_group_release(control)?;
                if self.prepared_group_release.is_none() {
                    let _ = self.refresh_power_save_demand(control)?;
                }
                airtime_result?;
            }
            return Ok(progress);
        }

        let phase = self
            .aggregate_phase
            .expect("ordinary service returned above");
        let action = phase.action(wake, || {
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
            })?;
            Ok(ordinary.now_micros())
        })?;
        let service_event = match action {
            AggregateServiceAction::Wait => return Ok(WifiTxProgress::Pending),
            AggregateServiceAction::Observe(event) => event,
            AggregateServiceAction::FinishAbort => {
                let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                    AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
                })?;
                aggregate
                    .active_mut()
                    .finish_timeout_abort(hardware)
                    .map_err(AccessPointDatapathError::Aggregate)?;
                ordinary.reset_aggregate_contention();
                self.aggregate_phase = None;
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.exchange_started_micros = None;
                }
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.observer {
                    observer.observe(AggregateTxObservation::HardwareTimeout);
                    observer.observe(AggregateTxObservation::WorkCompleted {
                        work: aggregate.active_mut().work(),
                    });
                }
                if let Some(accounting) = self.airtime.as_mut() {
                    accounting.complete_active(aggregate.active_mut().work())?;
                }
                return Ok(WifiTxProgress::Complete);
            }
        };
        if service_event == AggregateTxServiceEvent::Collision {
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
            })?;
            if !aggregate
                .active_mut()
                .abort_collision(hardware)
                .map_err(AccessPointDatapathError::Aggregate)?
            {
                return Err(AccessPointDatapathError::Aggregate(
                    ApAmpduError::HardwareDidNotDetach,
                ));
            }
            ordinary.reset_aggregate_contention();
            self.aggregate_phase = None;
            #[cfg(any(feature = "diagnostics", test))]
            {
                self.exchange_started_micros = None;
            }
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::Collision);
                observer.observe(AggregateTxObservation::WorkCompleted {
                    work: aggregate.active_mut().work(),
                });
            }
            if let Some(accounting) = self.airtime.as_mut() {
                accounting.complete_active(aggregate.active_mut().work())?;
            }
            return Ok(WifiTxProgress::Complete);
        }
        if matches!(
            service_event,
            AggregateTxServiceEvent::HardwareTimeout | AggregateTxServiceEvent::ExecutorDeadline
        ) {
            if !aggregate
                .active_mut()
                .begin_timeout_abort(hardware)
                .map_err(AccessPointDatapathError::Aggregate)?
            {
                return Err(AccessPointDatapathError::Aggregate(
                    ApAmpduError::HardwareDidNotDetach,
                ));
            }
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
            })?;
            // Sample after the abort request; no wait future owns this phase.
            self.aggregate_phase = Some(AggregateServicePhase::ResetRequired);
            self.aggregate_phase = Some(AggregateServicePhase::after_abort(ordinary.now_micros())?);
            return Ok(WifiTxProgress::Pending);
        }

        let aggregate_progress = {
            #[cfg(any(feature = "diagnostics", test))]
            let completion_started = self.observer.map(AggregateTxObserver::now_micros);
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
            })?;
            let progress = aggregate
                .active_mut()
                .service_completion(ordinary, hardware)
                .map_err(AccessPointDatapathError::Aggregate)?;
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                let finished = observer.now_micros();
                let started = completion_started.unwrap_or(finished);
                match progress {
                    ApAmpduProgress::Republished(_) => {
                        observer.observe(AggregateTxObservation::Published {
                            at_micros: started,
                            program_micros: finished.saturating_sub(started),
                        });
                    }
                    ApAmpduProgress::CompletionReady(_) => {
                        observer.observe(AggregateTxObservation::CompletionCoreCompleted {
                            micros: finished.saturating_sub(started),
                        });
                    }
                    ApAmpduProgress::Pending => {}
                }
            }
            progress
        };
        match aggregate_progress {
            ApAmpduProgress::CompletionReady(completion) => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observe_completion_details(completion, false);
                    if let Some(observer) = self.observer {
                        observer.observe(AggregateTxObservation::WorkCompleted {
                            work: aggregate.active_mut().work(),
                        });
                    }
                }
                #[cfg(not(any(feature = "diagnostics", test)))]
                let _ = completion;
                #[cfg(any(feature = "diagnostics", test))]
                let release_started = self.observer.map(AggregateTxObserver::now_micros);
                aggregate
                    .active_mut()
                    .release_completed()
                    .map_err(AccessPointDatapathError::Aggregate)?;
                self.aggregate_phase = None;
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.observer {
                    let finished = observer.now_micros();
                    observer.observe(AggregateTxObservation::BackingReleaseCompleted {
                        micros: finished.saturating_sub(release_started.unwrap_or(finished)),
                    });
                }
                #[cfg(any(feature = "diagnostics", test))]
                {
                    debug_assert!(self.terminal_acknowledged.is_none());
                    self.terminal_acknowledged = Some(completion.acknowledged);
                }
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.exchange_started_micros = None;
                }
                if let Some(accounting) = self.airtime.as_mut() {
                    accounting.complete_active(aggregate.active_mut().work())?;
                }
                Ok(WifiTxProgress::Complete)
            }
            ApAmpduProgress::Republished(completion) => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe_completion_details(completion, true);
                #[cfg(not(any(feature = "diagnostics", test)))]
                let _ = completion;
                let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                    AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
                })?;
                self.aggregate_phase = Some(AggregateServicePhase::Published(
                    ordinary
                        .now_micros()
                        .saturating_add(ordinary.publication_timeout_micros()),
                ));
                Ok(WifiTxProgress::Pending)
            }
            ApAmpduProgress::Pending => {
                if service_event == AggregateTxServiceEvent::Completion {
                    return Err(AccessPointDatapathError::Aggregate(
                        ApAmpduError::CompletionInterruptWithoutState,
                    ));
                }
                Ok(WifiTxProgress::Pending)
            }
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn observe_completion_details(&self, completion: ApAmpduCompletion, republished: bool) {
        let Some(observer) = self.observer else {
            return;
        };
        observer.observe(AggregateTxObservation::BlockAckProcessed {
            tx_status: completion.tx_status,
            block_ack_received: completion.block_ack_received,
            control: completion.block_ack_control,
            first_sequence: completion.first_sequence,
            starting_sequence: completion.starting_sequence,
            subframes: completion.subframes,
            missing: completion.missing,
        });
        if !republished && let Some(started) = self.exchange_started_micros {
            observer.observe(AggregateTxObservation::ExchangeCompleted {
                micros: observer.now_micros().saturating_sub(started),
                publications: completion.aggregate_attempts,
            });
        }
    }
}
