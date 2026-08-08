//! Cooperative control plane for one long-lived monitor task.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};

const MONITOR_IDLE: u8 = 0;
const MONITOR_RUNNING: u8 = 1;
const MONITOR_STOPPED: u8 = 2;
const MONITOR_FAULTED: u8 = 3;

/// A faulted control domain is sticky until board/radio reset reinitializes
/// its static storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MonitorControlError {
    ResetRequired,
}

/// Terminal state acknowledged by the task which owns IRQ and DMA.
///
/// `Stopped` means both the interrupt epoch and RX walker were confirmed
/// inactive. `Faulted` means the task retained the hardware owner for an
/// explicit radio reset; it never means that active resources were released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MonitorCompletion {
    Stopped,
    Faulted,
}

impl Esp32s31MonitorCompletion {
    const fn encode(self) -> u8 {
        match self {
            Self::Stopped => MONITOR_STOPPED,
            Self::Faulted => MONITOR_FAULTED,
        }
    }

    const fn decode(value: u8) -> Option<Self> {
        match value {
            MONITOR_STOPPED => Some(Self::Stopped),
            MONITOR_FAULTED => Some(Self::Faulted),
            _ => None,
        }
    }
}

/// Static mailbox shared by one application handle and one monitor task.
///
/// Hardware is deliberately absent from this value. Dropping the controller
/// cannot affect IRQ routing, DMA descriptors or their backing storage.
pub struct Esp32s31MonitorControlResources<M: RawMutex> {
    stop_requested: AtomicBool,
    completion: AtomicU8,
    command_wake: Signal<M, ()>,
    completion_wake: Signal<M, ()>,
}

impl<M: RawMutex> Esp32s31MonitorControlResources<M> {
    pub const fn new() -> Self {
        Self {
            stop_requested: AtomicBool::new(false),
            completion: AtomicU8::new(MONITOR_IDLE),
            command_wake: Signal::new(),
            completion_wake: Signal::new(),
        }
    }

    /// Begin one monitor task epoch.
    ///
    /// The mutable borrow proves that two command consumers cannot be created
    /// for the same mailbox. Both returned endpoints may live for the complete
    /// task epoch; only the runner endpoint can acknowledge completion.
    pub fn split(
        &mut self,
    ) -> Result<
        (
            Esp32s31MonitorController<'_, M>,
            Esp32s31MonitorCommandReceiver<'_, M>,
        ),
        Esp32s31MonitorControlError,
    > {
        self.ensure_reusable()?;
        Ok(self.split_checked())
    }

    pub(crate) fn ensure_reusable(&self) -> Result<(), Esp32s31MonitorControlError> {
        if self.completion.load(Ordering::Acquire) == MONITOR_FAULTED {
            Err(Esp32s31MonitorControlError::ResetRequired)
        } else {
            Ok(())
        }
    }

    pub(crate) fn split_checked(
        &mut self,
    ) -> (
        Esp32s31MonitorController<'_, M>,
        Esp32s31MonitorCommandReceiver<'_, M>,
    ) {
        debug_assert!(self.ensure_reusable().is_ok());
        self.stop_requested.store(false, Ordering::Release);
        self.completion.store(MONITOR_RUNNING, Ordering::Release);
        self.command_wake.reset();
        self.completion_wake.reset();
        let resources = &*self;
        (
            Esp32s31MonitorController { resources },
            Esp32s31MonitorCommandReceiver {
                resources,
                completion_published: false,
            },
        )
    }
}

impl<M: RawMutex> Default for Esp32s31MonitorControlResources<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Application-side handle for a monitor task.
///
/// A stop request is only publication. [`stop`](Self::stop) additionally waits
/// for the task to confirm that no ISR or DMA walker owns role resources.
pub struct Esp32s31MonitorController<'resources, M: RawMutex> {
    resources: &'resources Esp32s31MonitorControlResources<M>,
}

impl<M: RawMutex> Esp32s31MonitorController<'_, M> {
    pub fn request_stop(&self) -> bool {
        if self.resources.stop_requested.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.resources.command_wake.signal(());
        true
    }

    pub async fn wait_completion(&mut self) -> Esp32s31MonitorCompletion {
        loop {
            if let Some(completion) =
                Esp32s31MonitorCompletion::decode(self.resources.completion.load(Ordering::Acquire))
            {
                return completion;
            }
            self.resources.completion_wake.wait().await;
        }
    }

    pub async fn stop(&mut self) -> Esp32s31MonitorCompletion {
        self.request_stop();
        self.wait_completion().await
    }
}

/// Single-consumer endpoint owned by the monitor task.
pub struct Esp32s31MonitorCommandReceiver<'resources, M: RawMutex> {
    resources: &'resources Esp32s31MonitorControlResources<M>,
    completion_published: bool,
}

impl<M: RawMutex> Esp32s31MonitorCommandReceiver<'_, M> {
    pub async fn wait_stop(&mut self) {
        loop {
            if self.resources.stop_requested.load(Ordering::Acquire) {
                return;
            }
            self.resources.command_wake.wait().await;
        }
    }

    pub(crate) fn complete(&mut self, completion: Esp32s31MonitorCompletion) {
        if self.completion_published {
            return;
        }
        self.resources
            .completion
            .store(completion.encode(), Ordering::Release);
        self.completion_published = true;
        self.resources.completion_wake.signal(());
    }
}

impl<M: RawMutex> Drop for Esp32s31MonitorCommandReceiver<'_, M> {
    fn drop(&mut self) {
        if !self.completion_published {
            self.complete(Esp32s31MonitorCompletion::Faulted);
        }
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::*;

    #[test]
    fn request_is_idempotent_and_task_acknowledges_the_stop_edge() {
        let mut resources = Esp32s31MonitorControlResources::<NoopRawMutex>::new();
        let (mut controller, mut receiver) = resources.split().unwrap();

        assert!(controller.request_stop());
        assert!(!controller.request_stop());
        block_on(receiver.wait_stop());
        receiver.complete(Esp32s31MonitorCompletion::Stopped);

        assert_eq!(
            block_on(controller.wait_completion()),
            Esp32s31MonitorCompletion::Stopped
        );
    }

    #[test]
    fn dropping_the_application_handle_does_not_cancel_the_task_endpoint() {
        let mut resources = Esp32s31MonitorControlResources::<NoopRawMutex>::new();
        let (controller, mut receiver) = resources.split().unwrap();

        drop(controller);
        assert!(!receiver.resources.stop_requested.load(Ordering::Acquire));
        receiver.complete(Esp32s31MonitorCompletion::Faulted);
        assert_eq!(
            Esp32s31MonitorCompletion::decode(
                receiver.resources.completion.load(Ordering::Acquire)
            ),
            Some(Esp32s31MonitorCompletion::Faulted)
        );
    }

    #[test]
    fn dropping_task_endpoint_publishes_sticky_fault() {
        let mut resources = Esp32s31MonitorControlResources::<NoopRawMutex>::new();
        let (mut controller, receiver) = resources.split().unwrap();

        drop(receiver);
        assert_eq!(
            block_on(controller.wait_completion()),
            Esp32s31MonitorCompletion::Faulted
        );
        drop(controller);
        assert!(matches!(
            resources.split(),
            Err(Esp32s31MonitorControlError::ResetRequired)
        ));
    }
}
