//! Each hart scans its own inactive IRQ stack and publishes a typed result.

use oer_hil_protocol::system::StackWatermark;

#[cfg_attr(not(feature = "bluetooth-radio"), allow(dead_code))]
pub(crate) fn current_irq_snapshot() -> Option<StackWatermark> {
    {
        let capacity = crate::psram_task_stack::irq_stack_bytes() as u32;
        let free = crate::psram_task_stack::current_hart_interrupt_stack_free_bytes() as u32;
        // Source checks compile this image without the runner's stack policy;
        // only runner-built images take snapshots, and those always carry it.
        #[allow(
            clippy::option_env_unwrap,
            reason = "a snapshot from an image built without the HIL runner's policy is a bug"
        )]
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
}
