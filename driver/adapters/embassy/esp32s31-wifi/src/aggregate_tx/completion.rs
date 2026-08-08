use super::*;

impl<
    'slot,
    'ampdu,
    'resources,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>
    Esp32s31ConnectedTx<
        'slot,
        'ampdu,
        'resources,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub async fn service<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        if matches!(wake, WifiTxWake::Interrupt { .. }) {
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::InterruptServiceStarted {
                    at_micros: self.ordinary.now_micros(),
                });
            }
        }
        let active = mem::replace(&mut self.active, ConnectedTxActive::Idle);
        match active {
            ConnectedTxActive::Idle => Err(AggregateTxError::InactiveTransaction),
            ConnectedTxActive::Ordinary => {
                let progress = self.ordinary.service(hardware, wake).await?;
                if progress == WifiTxProgress::Pending {
                    self.active = ConnectedTxActive::Ordinary;
                } else if let Some(mut aggregate) = self.pending_ordinary_retry.take() {
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
            ConnectedTxActive::Aggregate(active) => {
                self.service_aggregate(hardware, wake, active).await
            }
        }
    }

    async fn service_aggregate<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        mut active: AggregateActive<SLOTS>,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let interrupt_events = match wake {
            WifiTxWake::Interrupt { events } => events,
            WifiTxWake::Deadline => 0,
        };
        let tx_events =
            interrupt_events & (MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION);
        if tx_events.count_ones() > 1 {
            return self.reset_required(AggregateTxResetReason::ConflictingInterruptEvents(
                tx_events,
            ));
        }

        if let Some(completion) = self.ampdu.acknowledge_completion(hardware)? {
            let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
            self.ampdu.detach_completed(hardware, cookie)?;
            let current_subframes = self.ampdu.frame_count();
            let decision = active.retry.observe(completion, current_subframes)?;
            if let AmpduRetryDecision::RetainAggregate { retry_mask } = decision {
                let aggregate = self.ampdu.retain_for_ampdu_retry(cookie, retry_mask)?;
                self.ordinary
                    .record_retry_failure(LegacyTxQueue::BestEffort);
                let (_, contention_window) = self
                    .ordinary
                    .contention_publication(LegacyTxQueue::BestEffort);
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
            if missing == 0 {
                self.ordinary.record_success(LegacyTxQueue::BestEffort);
            } else {
                self.ordinary
                    .reset_terminal_exchange(LegacyTxQueue::BestEffort);
            }

            let individual_retry = matches!(active.config, AmpduTxConfig::Ht(_))
                && missing == 1
                && active.retry.aggregate_attempts() < self.config.attempt_limit;
            if individual_retry {
                let index = retry_mask.trailing_zeros() as u8;
                let (frame_length, hardware_mic_length) = {
                    let (encoded, mic) = self.ampdu.completed_frame(cookie, index)?;
                    (self.ordinary.copy_encoded_retry(encoded)?, usize::from(mic))
                };
                self.release_completed()?;
                let progress = self.ordinary.start_prepared_encoded_retry(
                    hardware,
                    frame_length,
                    hardware_mic_length,
                    active.config.rate(),
                )?;
                self.pending_ordinary_retry = Some(MacAmpduTxStatus {
                    result: MacAmpduTxResult::Incomplete,
                    original_subframes: u16::from(active.original_subframes),
                    aggregate_attempts: active.retry.aggregate_attempts(),
                    aggregate_rate: active.config.rate(),
                    block_acknowledged_subframes: u16::from(active.retry.acknowledged()),
                    ordinary_retry: None,
                });
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
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::Completed {
                    acknowledged: active.retry.acknowledged(),
                    individual_retry: false,
                });
                Self::record_exchange_time(observer, &active, self.ordinary.now_micros());
            }
            return Ok(WifiTxProgress::Complete);
        }

        if tx_events == MAC_INT_TX_COMPLETE {
            return self.reset_required(AggregateTxResetReason::CompletionInterruptWithoutState);
        }
        if tx_events == MAC_INT_TX_TIMEOUT || matches!(wake, WifiTxWake::Deadline) {
            let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
            if !self.ampdu.begin_timeout_abort(hardware, cookie)? {
                return self.reset_required(if matches!(wake, WifiTxWake::Deadline) {
                    AggregateTxResetReason::ExecutorDeadline
                } else {
                    AggregateTxResetReason::TimeoutInterruptWithoutState
                });
            }
            self.ordinary.after_micros(AMPDU_ABORT_SETTLE_US).await;
            self.ampdu.finish_timeout_abort(hardware, cookie)?;
            self.release_frames();
            self.cookie = None;
            self.ordinary
                .reset_terminal_exchange(LegacyTxQueue::BestEffort);
            self.last_aggregate_status = Some(MacAmpduTxStatus {
                result: MacAmpduTxResult::HardwareTimeout,
                original_subframes: u16::from(active.original_subframes),
                aggregate_attempts: active.retry.aggregate_attempts(),
                aggregate_rate: active.config.rate(),
                block_acknowledged_subframes: u16::from(active.retry.acknowledged()),
                ordinary_retry: None,
            });
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::HardwareTimeout);
                Self::record_exchange_time(observer, &active, self.ordinary.now_micros());
            }
            return Ok(WifiTxProgress::Complete);
        }
        if tx_events == MAC_INT_COLLISION {
            let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
            if !self.ampdu.abort_collision(hardware, cookie)? {
                return self.reset_required(AggregateTxResetReason::CollisionInterruptWithoutState);
            }
            self.release_frames();
            self.cookie = None;
            self.ordinary
                .reset_terminal_exchange(LegacyTxQueue::BestEffort);
            self.last_aggregate_status = Some(MacAmpduTxStatus {
                result: MacAmpduTxResult::CollisionLimit,
                original_subframes: u16::from(active.original_subframes),
                aggregate_attempts: active.retry.aggregate_attempts(),
                aggregate_rate: active.config.rate(),
                block_acknowledged_subframes: u16::from(active.retry.acknowledged()),
                ordinary_retry: None,
            });
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::Collision);
                Self::record_exchange_time(observer, &active, self.ordinary.now_micros());
            }
            return Ok(WifiTxProgress::Complete);
        }

        self.active = ConnectedTxActive::Aggregate(active);
        Ok(WifiTxProgress::Pending)
    }

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
