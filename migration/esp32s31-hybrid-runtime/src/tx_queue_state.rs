//! Target-independent ownership primitive for strict logical TX queues.
//!
//! The queue owns foreign ESF frame pointers, but all intrusive-link access is
//! confined to these four unsafe operations. Scheduling policy remains a pure
//! function and can be verified on the host.

use core::ptr;

pub(crate) const TX_FRAME_NEXT_OFFSET: usize = 0x30;
pub(crate) const TXOP_CLASS_COUNT: usize = 3;

/// Exact three-byte TXOP allocator state published through the S31 ROM ABI.
///
/// The pinned `libpp.a[lmac.o]` object initializes the bytes to `[1, 1, 1]`.
/// `lmacRequestTxopQueue` takes the first non-zero byte and clears it;
/// `lmacReleaseTxopQueue` restores that byte to one. Keeping the recovered byte
/// representation lets the compatibility pointer refer directly to
/// Rust-owned storage instead of maintaining a shadow C object.
#[repr(C)]
pub(crate) struct TxopQueueState {
    available: [u8; TXOP_CLASS_COUNT],
}

impl TxopQueueState {
    pub(crate) const fn all_available() -> Self {
        Self {
            available: [1; TXOP_CLASS_COUNT],
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn reset(&mut self) {
        self.available = [1; TXOP_CLASS_COUNT];
    }

    pub(crate) fn request(&mut self) -> Option<u8> {
        let mut class = 0_usize;
        while class < TXOP_CLASS_COUNT {
            if self.available[class] != 0 {
                self.available[class] = 0;
                return Some(class as u8);
            }
            class += 1;
        }
        None
    }

    pub(crate) fn release(&mut self, class: u8) -> bool {
        let Some(available) = self.available.get_mut(usize::from(class)) else {
            return false;
        };
        *available = 1;
        true
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LogicalQueue {
    pub(crate) head: *mut u8,
    pub(crate) tail: *mut u8,
    pub(crate) selected: bool,
}

impl LogicalQueue {
    pub(crate) const fn empty() -> Self {
        Self {
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
            selected: false,
        }
    }

    pub(crate) unsafe fn append(&mut self, frame: *mut u8) -> bool {
        if frame.is_null()
            || !frame
                .add(TX_FRAME_NEXT_OFFSET)
                .cast::<*mut u8>()
                .read()
                .is_null()
        {
            return false;
        }
        if self.tail.is_null() {
            if !self.head.is_null() {
                return false;
            }
            self.head = frame;
        } else {
            self.tail
                .add(TX_FRAME_NEXT_OFFSET)
                .cast::<*mut u8>()
                .write(frame);
        }
        self.tail = frame;
        true
    }

    pub(crate) unsafe fn pop_front(&mut self) -> *mut u8 {
        let frame = self.head;
        if frame.is_null() {
            return frame;
        }
        self.head = frame.add(TX_FRAME_NEXT_OFFSET).cast::<*mut u8>().read();
        if self.head.is_null() {
            self.tail = ptr::null_mut();
        }
        frame
            .add(TX_FRAME_NEXT_OFFSET)
            .cast::<*mut u8>()
            .write(ptr::null_mut());
        frame
    }

    pub(crate) unsafe fn push_front(&mut self, frame: *mut u8) -> bool {
        if frame.is_null() {
            return false;
        }
        frame
            .add(TX_FRAME_NEXT_OFFSET)
            .cast::<*mut u8>()
            .write(self.head);
        self.head = frame;
        if self.tail.is_null() {
            self.tail = frame;
        }
        true
    }

    pub(crate) unsafe fn prepend_chain(&mut self, head: *mut u8, tail: *mut u8) -> bool {
        if head.is_null() || tail.is_null() {
            return false;
        }
        tail.add(TX_FRAME_NEXT_OFFSET)
            .cast::<*mut u8>()
            .write(self.head);
        if self.tail.is_null() {
            self.tail = tail;
        }
        self.head = head;
        true
    }
}

pub(crate) const fn select_ready_logical_queue(
    hardware_queue: u8,
    allowed_mask: u16,
    cursor: u8,
    ready_mask: u16,
    advance: bool,
) -> Option<u8> {
    let candidates = allowed_mask & ready_mask;
    if !advance && cursor < 16 && candidates & (1_u16 << cursor) != 0 {
        return Some(cursor);
    }
    let mut offset = 1_u8;
    while offset <= 16 {
        let logical_queue = cursor.wrapping_add(offset) & 0x0f;
        if candidates & (1_u16 << logical_queue) != 0 {
            return Some(logical_queue);
        }
        offset += 1;
    }
    // The pinned event-zero selector has an explicit latency fallback over
    // logical queues 0..=2 when its scheduled bitmap has no ready member.
    if hardware_queue == 0 {
        let fallback = ready_mask & 0x0007;
        if fallback != 0 {
            return Some(fallback.trailing_zeros() as u8);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        select_ready_logical_queue, LogicalQueue, TxopQueueState, TX_FRAME_NEXT_OFFSET,
    };

    unsafe fn next(frame: *mut u8) -> *mut u8 {
        frame.add(TX_FRAME_NEXT_OFFSET).cast::<*mut u8>().read()
    }

    #[test]
    fn owns_append_pop_and_front_requeue_order() {
        let mut first = [0_usize; 8];
        let mut second = [0_usize; 8];
        let mut retry = [0_usize; 8];
        let first = first.as_mut_ptr().cast::<u8>();
        let second = second.as_mut_ptr().cast::<u8>();
        let retry = retry.as_mut_ptr().cast::<u8>();
        let mut queue = LogicalQueue::empty();

        unsafe {
            assert!(queue.append(first));
            assert!(queue.append(second));
            assert_eq!(queue.head, first);
            assert_eq!(queue.tail, second);
            assert_eq!(next(first), second);
            assert!(next(second).is_null());

            assert_eq!(queue.pop_front(), first);
            assert!(next(first).is_null());
            assert!(queue.push_front(retry));
            assert_eq!(queue.pop_front(), retry);
            assert_eq!(queue.pop_front(), second);
            assert!(queue.pop_front().is_null());
            assert!(queue.head.is_null());
            assert!(queue.tail.is_null());
        }
    }

    #[test]
    fn timeout_chain_preserves_the_existing_tail() {
        let mut chain_head = [0_usize; 8];
        let mut chain_tail = [0_usize; 8];
        let mut existing = [0_usize; 8];
        let chain_head = chain_head.as_mut_ptr().cast::<u8>();
        let chain_tail = chain_tail.as_mut_ptr().cast::<u8>();
        let existing = existing.as_mut_ptr().cast::<u8>();
        let mut queue = LogicalQueue::empty();

        unsafe {
            chain_head
                .add(TX_FRAME_NEXT_OFFSET)
                .cast::<*mut u8>()
                .write(chain_tail);
            assert!(queue.append(existing));
            assert!(queue.prepend_chain(chain_head, chain_tail));
            assert_eq!(queue.head, chain_head);
            assert_eq!(queue.tail, existing);
            assert_eq!(next(chain_head), chain_tail);
            assert_eq!(next(chain_tail), existing);
        }
    }

    #[test]
    fn hardware_bitmap_selects_and_rotates_all_logical_queues() {
        assert_eq!(
            select_ready_logical_queue(1, 0x0013, 1, 0x0010, false),
            Some(4)
        );
        assert_eq!(
            select_ready_logical_queue(1, 0x0013, 0, 0x0013, false),
            Some(0)
        );
        assert_eq!(
            select_ready_logical_queue(1, 0x0013, 0, 0x0013, true),
            Some(1)
        );
        assert_eq!(
            select_ready_logical_queue(1, 0x0013, 4, 0x0013, true),
            Some(0)
        );
        assert_eq!(select_ready_logical_queue(1, 0x0013, 4, 0x0004, true), None);
    }

    #[test]
    fn hardware_zero_preserves_the_recovered_latency_fallback() {
        assert_eq!(select_ready_logical_queue(0, 0, 0, 0x0004, false), Some(2));
        assert_eq!(select_ready_logical_queue(1, 0, 0, 0x0004, false), None);
    }

    #[test]
    fn txop_allocator_preserves_vendor_first_available_order() {
        let mut state = TxopQueueState::all_available();
        assert_eq!(state.request(), Some(0));
        assert_eq!(state.request(), Some(1));
        assert_eq!(state.request(), Some(2));
        assert_eq!(state.request(), None);
    }

    #[test]
    fn txop_release_is_idempotent_and_rejects_invalid_classes() {
        let mut state = TxopQueueState::all_available();
        assert_eq!(state.request(), Some(0));
        assert!(state.release(0));
        assert!(state.release(0));
        assert!(!state.release(3));
        assert_eq!(state.request(), Some(0));
        assert_eq!(state.request(), Some(1));
    }
}
