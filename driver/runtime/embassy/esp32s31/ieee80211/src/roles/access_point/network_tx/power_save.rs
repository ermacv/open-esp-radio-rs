//! Power-save admission and affine unicast/group release transactions.
//! The network TX owner retains every lease until commit, rollback or discard.

use super::*;

impl<'observer, B, N> Esp32s31AccessPointNetworkTx<'observer, B, N>
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
{
    pub(super) fn retain_power_save(
        &mut self,
        engine: &mut Esp32s31ApEngine<'_>,
        frame: N,
    ) -> Result<Option<(ApTxFlowKey, N)>, Esp32s31AccessPointDatapathError> {
        let unbound_key = ApTxFlowKey::unbound_from_ethernet(frame.as_slice());
        let Some(peer) = frame
            .as_slice()
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            return Ok(Some((unbound_key, frame)));
        };
        if peer[0] & 1 != 0 {
            if engine.group_downlink_disposition() == ApDownlinkDisposition::TransmitNow {
                return Ok(Some((unbound_key, frame)));
            }
            let Ok(index) = self.buffered_group.push(frame, &mut self.frame_arena) else {
                // The caller-owned queue is deliberately bounded. Releasing
                // this excess lease applies backpressure at the producer pool
                // without claiming a TIM entry for payload we did not retain.
                return Ok(None);
            };
            if let Err(error) = engine.commit_buffered_group() {
                let _ = self
                    .buffered_group
                    .take_at(index, &mut self.frame_arena)
                    .expect("the just-inserted AP group lease is still owned");
                return Err(Esp32s31AccessPointDatapathError::Control(
                    Esp32s31AccessPointControlError::from(error),
                ));
            }
            return Ok(None);
        }
        let admission = match engine.admit_downlink(peer) {
            Ok(admission) => admission,
            // Preserve the ordinary admission path for an unknown or
            // unauthorized destination so its existing rejection accounting
            // remains authoritative.
            Err(_) => return Ok(Some((unbound_key, frame))),
        };
        let identity = admission.identity();
        let key = ApTxFlowKey::associated(identity);
        if admission.disposition() == ApDownlinkDisposition::TransmitNow {
            return Ok(Some((key, frame)));
        }

        let Ok(index) = self
            .buffered_unicast
            .push(identity, frame, &mut self.frame_arena)
        else {
            // The bounded queue owns the complete default TX lease frontier.
            // A custom larger producer cannot force an allocation or an
            // unbounded retention path; its excess lease is released here.
            return Ok(None);
        };
        if let Err(error) = engine.commit_buffered_unicast(identity) {
            let _ = self
                .buffered_unicast
                .take_at(index, &mut self.frame_arena)
                .expect("the just-inserted AP power-save lease is still owned");
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::from(error),
            ));
        }
        Ok(None)
    }

    /// Reserve the oldest retained frame whose peer has returned to Active.
    /// This mutates no frame bytes and leaves the TIM count unchanged until
    /// terminal TX resolves the affine release token.
    pub(in super::super) fn stage_awake_buffered_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<bool, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if self.prepared_buffered_release.is_some() || self.active_buffered_release.is_some() {
            return Ok(false);
        }

        if let Some(release) = control.take_pending_buffered_release() {
            let identity = release.identity();
            if let Some(index) = self.buffered_unicast.oldest_index_for(identity) {
                let buffered = self
                    .buffered_unicast
                    .take_at(index, &mut self.frame_arena)
                    .expect("the PS-Poll release names one retained lease");
                self.prepared_buffered_release = Some(BufferedUnicastRelease { buffered, release });
                return Ok(true);
            }
            control
                .mac
                .engine_mut()
                .complete_buffered_unicast_release(release, false)
                .map_err(Esp32s31AccessPointControlError::from)
                .map_err(Esp32s31AccessPointDatapathError::Control)?;
        }

        // Peer teardown clears the portable counters. Release matching caller
        // leases at the same observation boundary instead of retaining stale
        // addresses into a later association generation.
        self.buffered_unicast
            .retain(&mut self.frame_arena, |identity| {
                control.mac.engine().association_is_current(identity)
            });
        let Some(identity) = self.buffered_unicast.oldest_releasable_peer(|identity| {
            control
                .mac
                .engine()
                .association_status(identity)
                .is_some_and(|status| {
                    status.power_state == ApPeerPowerState::Active
                        && !status.buffered_release_in_flight
                })
        }) else {
            return Ok(false);
        };
        let Some(release) = control
            .mac
            .engine_mut()
            .begin_buffered_unicast_release(identity)
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?
        else {
            return Ok(false);
        };
        let Some(index) = self.buffered_unicast.oldest_index_for(identity) else {
            let _ = control
                .mac
                .engine_mut()
                .complete_buffered_unicast_release(release, false);
            return Ok(false);
        };
        let buffered = self
            .buffered_unicast
            .take_at(index, &mut self.frame_arena)
            .expect("the selected AP power-save lease remains retained");
        self.prepared_buffered_release = Some(BufferedUnicastRelease { buffered, release });
        Ok(true)
    }

    pub(super) fn rollback_prepared_buffered_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(prepared) = self.prepared_buffered_release.take() else {
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_unicast_release(prepared.release, false);
        self.buffered_unicast
            .restore(prepared.buffered, &mut self.frame_arena);
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)
    }

    pub(super) fn complete_active_buffered_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        delivered: bool,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(active) = self.active_buffered_release.take() else {
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_unicast_release(active.release, delivered);
        if !delivered || result.is_err() {
            self.buffered_unicast
                .restore(active.buffered, &mut self.frame_arena);
        }
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
        let _ = self.stage_awake_buffered_release(control)?;
        Ok(())
    }

    pub(super) fn start_prepared_buffered_release<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
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
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware,
    {
        let prepared = self
            .prepared_buffered_release
            .take()
            .expect("checked prepared AP power-save release");
        let result = control.start_network_tx_with_more_data(
            hardware,
            prepared.buffered.frame.as_slice(),
            prepared.release.more_data(),
        );
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active_buffered_release = Some(prepared);
                Ok(WifiTxProgress::Pending)
            }
            Ok(WifiTxProgress::Complete) => {
                self.prepared_buffered_release = Some(prepared);
                self.rollback_prepared_buffered_release(control)?;
                Ok(WifiTxProgress::Complete)
            }
            Err(error) => {
                self.prepared_buffered_release = Some(prepared);
                self.rollback_prepared_buffered_release(control)?;
                Err(Esp32s31AccessPointDatapathError::Control(error))
            }
        }
    }

    /// Bind the exact queue prefix announced by a successfully transmitted
    /// DTIM beacon to the oldest caller-owned group lease.
    pub(super) fn stage_dtim_group_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<bool, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if let Some(advertised_frames) = control.take_pending_dtim_group_frames() {
            if self.dtim_group_release_remaining != 0
                || self.prepared_group_release.is_some()
                || self.active_group_release.is_some()
            {
                return Err(Esp32s31AccessPointDatapathError::Control(
                    Esp32s31AccessPointControlError::DtimGroupReleaseAlreadyPending,
                ));
            }
            self.dtim_group_release_remaining = advertised_frames;
        }
        if self.dtim_group_release_remaining == 0
            || self.prepared_group_release.is_some()
            || self.active_group_release.is_some()
        {
            return Ok(false);
        }

        let Some(release) = control
            .mac
            .engine_mut()
            .begin_buffered_group_release()
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?
        else {
            self.dtim_group_release_remaining = 0;
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        };
        let Some(index) = self.buffered_group.oldest_index() else {
            let rollback = control
                .mac
                .engine_mut()
                .complete_buffered_group_release(release, false)
                .map_err(Esp32s31AccessPointControlError::from)
                .map_err(Esp32s31AccessPointDatapathError::Control);
            self.dtim_group_release_remaining = 0;
            rollback?;
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        };
        let buffered = self
            .buffered_group
            .take_at(index, &mut self.frame_arena)
            .expect("the selected AP group lease remains retained");
        self.prepared_group_release = Some(BufferedGroupRelease { buffered, release });
        Ok(true)
    }

    fn rollback_prepared_group_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(prepared) = self.prepared_group_release.take() else {
            self.dtim_group_release_remaining = 0;
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_group_release(prepared.release, false);
        self.buffered_group
            .restore(prepared.buffered, &mut self.frame_arena);
        self.dtim_group_release_remaining = 0;
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)
    }

    pub(super) fn complete_active_group_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        published: bool,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(active) = self.active_group_release.take() else {
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_group_release(active.release, published);
        if !published || result.is_err() {
            self.buffered_group
                .restore(active.buffered, &mut self.frame_arena);
            self.dtim_group_release_remaining = 0;
        } else {
            self.dtim_group_release_remaining = self
                .dtim_group_release_remaining
                .checked_sub(1)
                .ok_or(Esp32s31AccessPointDatapathError::Control(
                    Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
                ))?;
        }
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
        if self.dtim_group_release_remaining != 0 {
            let _ = self.stage_dtim_group_release(control)?;
        }
        Ok(())
    }

    pub(super) fn start_prepared_group_release<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
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
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware,
    {
        let prepared = self
            .prepared_group_release
            .take()
            .expect("checked prepared AP DTIM group release");
        let result = control.start_network_tx_with_more_data(
            hardware,
            prepared.buffered.frame.as_slice(),
            prepared.release.more_data(),
        );
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active_group_release = Some(prepared);
                Ok(WifiTxProgress::Pending)
            }
            Ok(WifiTxProgress::Complete) => {
                self.prepared_group_release = Some(prepared);
                self.rollback_prepared_group_release(control)?;
                // The control owner returns Complete without publication when
                // no authorized receiver remains. Drop both the retained
                // leases and their TIM accounting instead of advertising an
                // undeliverable queue forever.
                self.discard_group_buffer(control)?;
                Ok(WifiTxProgress::Complete)
            }
            Err(error) => {
                self.prepared_group_release = Some(prepared);
                self.rollback_prepared_group_release(control)?;
                Err(Esp32s31AccessPointDatapathError::Control(error))
            }
        }
    }

    pub(in super::super) fn discard_group_buffer<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if self.active_group_release.is_some() {
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        }
        self.rollback_prepared_group_release(control)?;
        let _ = control.take_pending_dtim_group_frames();
        let portable = control
            .mac
            .engine_mut()
            .discard_buffered_groups()
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
        let retained = self.buffered_group.clear(&mut self.frame_arena);
        self.dtim_group_release_remaining = 0;
        if usize::from(portable) != retained {
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        }
        Ok(())
    }
}

impl<'observer, B, N, P, E, T, const DMA_BUFFER_SIZE: usize, const TX_BUFFER_SIZE: usize>
    AccessPointPowerSaveNetworkTx<P, E, T, DMA_BUFFER_SIZE, TX_BUFFER_SIZE>
    for Esp32s31AccessPointNetworkTx<'observer, B, N>
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    fn stage_awake_release(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<bool, Esp32s31AccessPointDatapathError> {
        self.stage_awake_buffered_release(control)
    }

    fn has_power_save_release(&self) -> bool {
        self.prepared_buffered_release.is_some()
            || self.active_buffered_release.is_some()
            || self.prepared_group_release.is_some()
            || self.active_group_release.is_some()
            || self.dtim_group_release_remaining != 0
    }

    fn discard_group_power_save(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError> {
        self.discard_group_buffer(control)
    }
}
