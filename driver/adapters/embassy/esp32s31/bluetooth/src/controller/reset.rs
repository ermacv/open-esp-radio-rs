//! Borrowed Reset-stopping waits that retain the Controller owner.

use super::*;

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) async fn wait_reset_stopping<WakeMutex, Recheck>(
        &mut self,
        wakers: &EmbassyBluetoothRuntimeWakers<WakeMutex>,
        recheck: &mut Recheck,
    ) where
        WakeMutex: RawMutex,
        Recheck: EmbassyBluetoothDtmControllerTimeRecheck,
    {
        let EmbassyBluetoothControllerCommandState::ResetStopping(runner) = self.owner.current()
        else {
            unreachable!("the selected Reset-stopping phase did not change")
        };
        match runner.wait() {
            Some(BluetoothDtmResetStoppingWait::Scheduler(wake)) => {
                wakers.wait_scheduler_ready(wake).await;
            }
            Some(BluetoothDtmResetStoppingWait::PostUnlink(wake)) => {
                let _ = wakers
                    .wait_post_unlink_or_recheck(wake, recheck.wait_until_absolute_recheck())
                    .await;
            }
            Some(BluetoothDtmResetStoppingWait::ControllerTime) => {
                recheck.wait_until_absolute_recheck().await;
            }
            None => {}
        }
    }
}
