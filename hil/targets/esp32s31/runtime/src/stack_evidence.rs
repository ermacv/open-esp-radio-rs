//! Each hart scans its own inactive IRQ stack and publishes a typed result.

use oer_hil_protocol::StackWatermark;

pub(crate) fn current_irq_snapshot() -> Option<StackWatermark> {
    #[cfg(feature = "psram-task-stack")]
    {
        let capacity = crate::psram_task_stack::IRQ_STACK_BYTES as u32;
        let free = crate::psram_task_stack::current_hart_interrupt_stack_free_bytes() as u32;
        let minimum = option_env!("OPEN_RADIO_IRQ_STACK_MINIMUM_FREE_BYTES")
            .expect("HIL runner must provide IRQ headroom policy")
            .parse::<u32>()
            .expect("unsigned IRQ headroom");
        Some(StackWatermark {
            capacity_bytes: capacity,
            free_bytes: free,
            used_bytes: capacity - free,
            minimum_free_bytes: minimum,
        })
    }
    #[cfg(not(feature = "psram-task-stack"))]
    None
}

#[cfg(feature = "open-radio-hil")]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

#[cfg(feature = "open-radio-hil")]
static REQUEST: Channel<CriticalSectionRawMutex, (), 1> = Channel::new();
#[cfg(feature = "open-radio-hil")]
static RESPONSE: Channel<CriticalSectionRawMutex, (StackWatermark, Option<StackWatermark>), 1> =
    Channel::new();

/// The console is the sole requester and awaits every response without cancellation.
/// A stuck CPU1 causes an explicit failure, never stale or foreign-stack evidence.
#[cfg(feature = "open-radio-hil")]
pub(crate) async fn cpu1_snapshot() -> (StackWatermark, Option<StackWatermark>) {
    embassy_time::with_timeout(embassy_time::Duration::from_secs(2), async {
        REQUEST.send(()).await;
        RESPONSE.receive().await
    })
    .await
    .expect("CPU1 IRQ stack sampler timed out")
}

#[cfg(feature = "open-radio-hil")]
#[embassy_executor::task]
pub(crate) async fn cpu1_sampler() {
    loop {
        REQUEST.receive().await;
        RESPONSE
            .send((crate::cpu1_stack_usage_snapshot(), current_irq_snapshot()))
            .await;
    }
}
