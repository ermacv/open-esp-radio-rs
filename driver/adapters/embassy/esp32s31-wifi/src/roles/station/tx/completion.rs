use super::*;

impl<
    'slot,
    'ampdu,
    B,
    P,
    E,
    T,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> Esp32s31ConnectedTx<'slot, 'ampdu, B, P, E, T, SLOTS, AMPDU_BUFFER_SIZE, ORDINARY_BUFFER_SIZE>
where
    B: MaterializedTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    /// Service one captured event synchronously. Pending timeout-abort keeps
    /// the aggregate owners in this state machine, never in a suspended future.
    /// The executor or fused owner uses `next_deadline_micros` to wait.
    pub fn service<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        #[cfg(any(feature = "diagnostics", test))]
        if matches!(wake, WifiTxWake::Interrupt { .. })
            && let Some(observer) = self.observer
        {
            observer.observe(AggregateTxObservation::InterruptServiceStarted {
                at_micros: self.ordinary.now_micros(),
            });
        }
        let active = mem::replace(&mut self.active, ConnectedTxActive::Idle);
        match active {
            ConnectedTxActive::Idle => Err(AggregateTxError::InactiveTransaction),
            ConnectedTxActive::Ordinary => {
                let progress = self.ordinary.service(hardware, wake)?;
                if progress == WifiTxProgress::Pending {
                    self.active = ConnectedTxActive::Ordinary;
                } else {
                    self.observe_ordinary_rate_control();
                }
                if progress != WifiTxProgress::Pending
                    && let Some(mut aggregate) = self.pending_ordinary_retry.take()
                {
                    let ordinary = self
                        .ordinary
                        .last_outcome()
                        .ok_or(AggregateTxError::MissingOrdinaryRetryStatus)?;
                    let ordinary = ordinary.report().status;
                    aggregate.result = if matches!(ordinary.result, MacTxResult::Transmitted) {
                        MacAmpduTxResult::Delivered
                    } else {
                        MacAmpduTxResult::Incomplete
                    };
                    aggregate.ordinary_retry = Some(ordinary);
                    self.last_aggregate_status = Some(aggregate);
                }
                Ok(progress)
            }
            ConnectedTxActive::AbortSettling(active) => self.service_abort_settle(hardware, active),
            ConnectedTxActive::Aggregate(active) => self.service_aggregate(hardware, wake, active),
        }
    }

    fn service_abort_settle<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        active: AggregateActive<SLOTS>,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        if self.ordinary.now_micros() < active.deadline_micros {
            self.active = ConnectedTxActive::AbortSettling(active);
            return Ok(WifiTxProgress::Pending);
        }
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        self.ampdu
            .active_mut()
            .finish_timeout_abort(hardware, cookie)?;
        self.release_frames();
        self.cookie = None;
        self.ordinary
            .reset_terminal_exchange(active.traffic.queue());
        self.last_aggregate_status = Some(MacAmpduTxStatus {
            result: MacAmpduTxResult::HardwareTimeout,
            original_subframes: u16::from(active.original_subframes),
            aggregate_attempts: active.retry.aggregate_attempts(),
            aggregate_rate: active.config.rate(),
            block_acknowledged_subframes: u16::from(active.retry.acknowledged()),
            ordinary_retry: None,
        });
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::HardwareTimeout);
            Self::record_exchange_time(observer, &active, self.ordinary.now_micros());
        }
        Ok(WifiTxProgress::Complete)
    }

    fn service_aggregate<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        mut active: AggregateActive<SLOTS>,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let service_event = match AggregateTxServiceEvent::classify(wake) {
            Ok(event) => event,
            Err(error) => {
                return self.reset_required(AggregateTxResetReason::ConflictingInterruptEvents(
                    error.events,
                ));
            }
        };

        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        if let Some(observed) =
            self.ampdu
                .active_mut()
                .observe_retry_completion(hardware, cookie, &mut active.retry)?
        {
            let completion = observed.completion;
            let current_subframes = observed.subframes;
            #[cfg(any(feature = "diagnostics", test))]
            let current_first_sequence = observed.first_sequence;
            let decision = observed.decision;
            self.rate_control.observe_tx_completion(completion.tx);
            // The retry owner has already validated both counts. An absent
            // A-MPDU rate arena disables adaptation but cannot alter DMA or
            // retry ownership; malformed counts are therefore impossible at
            // this typed boundary and remain diagnostic-only.
            let acknowledged = current_subframes.saturating_sub(decision.missing());
            let observation = self.rate_control.observe_ampdu_block_ack(
                self.ordinary.now_micros() as u32,
                u16::from(current_subframes),
                u16::from(acknowledged),
            );
            debug_assert!(matches!(
                observation,
                Ok(_) | Err(AmpduRateObservationError::Unavailable)
            ));
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::BlockAckProcessed {
                    tx_status: completion.tx.status(),
                    block_ack_received: completion.block_ack_received,
                    control: completion.block_ack.control,
                    first_sequence: current_first_sequence,
                    starting_sequence: completion.block_ack.block_ack.starting_sequence,
                    subframes: current_subframes,
                    missing: decision.missing(),
                });
            }
            if let AmpduRetryDecision::RetainAggregate { retry_mask } = decision {
                let queue = active.traffic.queue();
                let aggregate = self
                    .ampdu
                    .active_mut()
                    .retain_for_ampdu_retry(cookie, retry_mask)?;
                self.ordinary.record_retry_failure(queue);
                let (_, contention_window) = self.ordinary.contention_publication(queue);
                active.config.update_retained_retry(
                    aggregate.bytes,
                    aggregate.subframes,
                    contention_window,
                );
                if let Err(error) = self.publish_attempt(hardware, &mut active) {
                    self.cancel_prepared();
                    return Err(error);
                }
                self.active = ConnectedTxActive::Aggregate(active);
                return Ok(WifiTxProgress::Pending);
            }

            let retry_mask = decision.retry_mask();
            let missing = decision.missing();
            let queue = active.traffic.queue();
            if missing == 0 {
                self.ordinary.record_success(queue);
            } else {
                self.ordinary.reset_terminal_exchange(queue);
            }

            let individual_retry = matches!(active.config, AmpduTxConfig::Ht(_))
                && missing == 1
                && active.retry.aggregate_attempts() < self.config.attempt_limit;
            if individual_retry {
                let index = retry_mask.trailing_zeros() as u8;
                let (frame_length, hardware_mic_length) = {
                    let (encoded, mic) = self.ampdu.active_mut().completed_frame(cookie, index)?;
                    (self.ordinary.copy_encoded_retry(encoded)?, usize::from(mic))
                };
                self.release_completed()?;
                let progress = self.ordinary.start_prepared_encoded_retry_for_category(
                    hardware,
                    frame_length,
                    hardware_mic_length,
                    active.config.rate(),
                    active.traffic.selected.access_category,
                )?;
                self.pending_ordinary_retry = Some(MacAmpduTxStatus {
                    result: MacAmpduTxResult::Incomplete,
                    original_subframes: u16::from(active.original_subframes),
                    aggregate_attempts: active.retry.aggregate_attempts(),
                    aggregate_rate: active.config.rate(),
                    block_acknowledged_subframes: u16::from(active.retry.acknowledged()),
                    ordinary_retry: None,
                });
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.observer {
                    observer.observe(AggregateTxObservation::Completed {
                        acknowledged: active.retry.acknowledged(),
                        individual_retry: true,
                    });
                    Self::record_exchange_time(observer, &active, self.ordinary.now_micros());
                }
                self.active = ConnectedTxActive::Ordinary;
                return Ok(progress);
            }

            self.release_completed()?;
            let acknowledged = active.retry.acknowledged();
            self.last_aggregate_status = Some(MacAmpduTxStatus {
                result: if acknowledged == active.original_subframes {
                    MacAmpduTxResult::Delivered
                } else {
                    MacAmpduTxResult::Incomplete
                },
                original_subframes: u16::from(active.original_subframes),
                aggregate_attempts: active.retry.aggregate_attempts(),
                aggregate_rate: active.config.rate(),
                block_acknowledged_subframes: u16::from(acknowledged),
                ordinary_retry: None,
            });
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::Completed {
                    acknowledged: active.retry.acknowledged(),
                    individual_retry: false,
                });
                Self::record_exchange_time(observer, &active, self.ordinary.now_micros());
            }
            return Ok(WifiTxProgress::Complete);
        }

        if service_event == AggregateTxServiceEvent::Completion {
            return self.reset_required(AggregateTxResetReason::CompletionInterruptWithoutState);
        }
        if matches!(
            service_event,
            AggregateTxServiceEvent::HardwareTimeout | AggregateTxServiceEvent::ExecutorDeadline
        ) {
            if service_event == AggregateTxServiceEvent::ExecutorDeadline
                && self.ordinary.now_micros() < active.deadline_micros
            {
                self.active = ConnectedTxActive::Aggregate(active);
                return Ok(WifiTxProgress::Pending);
            }
            let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
            if !self
                .ampdu
                .active_mut()
                .begin_timeout_abort(hardware, cookie)?
            {
                return self.reset_required(
                    if service_event == AggregateTxServiceEvent::ExecutorDeadline {
                        AggregateTxResetReason::ExecutorDeadline
                    } else {
                        AggregateTxResetReason::TimeoutInterruptWithoutState
                    },
                );
            }
            let Some(deadline_micros) = self
                .ordinary
                .now_micros()
                .checked_add(AMPDU_ABORT_SETTLE_US)
            else {
                self.ampdu.active_mut().require_reset(cookie)?;
                return Err(AggregateTxError::DeadlineOverflow);
            };
            active.deadline_micros = deadline_micros;
            self.active = ConnectedTxActive::AbortSettling(active);
            return Ok(WifiTxProgress::Pending);
        }
        if service_event == AggregateTxServiceEvent::Collision {
            let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
            if !self.ampdu.active_mut().abort_collision(hardware, cookie)? {
                return self.reset_required(AggregateTxResetReason::CollisionInterruptWithoutState);
            }
            self.release_frames();
            self.cookie = None;
            self.ordinary
                .reset_terminal_exchange(active.traffic.queue());
            self.last_aggregate_status = Some(MacAmpduTxStatus {
                result: MacAmpduTxResult::CollisionLimit,
                original_subframes: u16::from(active.original_subframes),
                aggregate_attempts: active.retry.aggregate_attempts(),
                aggregate_rate: active.config.rate(),
                block_acknowledged_subframes: u16::from(active.retry.acknowledged()),
                ordinary_retry: None,
            });
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::Collision);
                Self::record_exchange_time(observer, &active, self.ordinary.now_micros());
            }
            return Ok(WifiTxProgress::Complete);
        }

        self.active = ConnectedTxActive::Aggregate(active);
        Ok(WifiTxProgress::Pending)
    }

    fn observe_ordinary_rate_control(&mut self) {
        let Some(outcome) = self.ordinary.last_outcome() else {
            return;
        };
        let report = outcome.report();
        if let Some(completion) = report.completion {
            self.rate_control.observe_tx_completion(completion);
        }
        self.rate_control
            .update_tx_per(u32::from(report.status.attempts.saturating_sub(1)));
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn record_exchange_time(
        observer: &dyn AggregateTxObserver,
        active: &AggregateActive<SLOTS>,
        finished_micros: u64,
    ) {
        if let Some(started_micros) = active.first_publication_micros {
            observer.observe(AggregateTxObservation::ExchangeCompleted {
                micros: finished_micros.wrapping_sub(started_micros),
                publications: active.retry.aggregate_attempts(),
            });
        }
    }
}
