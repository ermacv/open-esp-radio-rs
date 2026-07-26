use core::{
    ffi::{c_char, c_void},
    slice,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::channel::{BoundedChannel, Receive};

pub const WIFI_EVENT_BASE_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventCopyError {
    NullBase,
    BaseTooLong,
    NullData,
    DataTooLarge(usize),
    ChannelFull,
}

/// An event whose base name and payload are owned by Rust storage.
///
/// No pointer received from the vendor is retained after `_event_post`
/// returns, so stack-backed and temporary C payloads are safe to consume from
/// another async poll.
#[derive(Clone)]
pub struct OwnedWifiEvent<const DATA: usize> {
    base: [u8; WIFI_EVENT_BASE_CAPACITY],
    base_len: usize,
    id: i32,
    data: [u8; DATA],
    data_len: usize,
}

impl<const DATA: usize> OwnedWifiEvent<DATA> {
    pub fn base(&self) -> &[u8] {
        &self.base[..self.base_len]
    }

    pub const fn id(&self) -> i32 {
        self.id
    }

    pub fn data(&self) -> &[u8] {
        &self.data[..self.data_len]
    }

    /// Copy an event directly from the synchronous OSI callback arguments.
    ///
    /// # Safety
    /// `event_base` must reference a NUL-terminated string and `event_data`
    /// must reference `event_data_size` readable bytes when that size is not
    /// zero. Both only need to remain valid for this call.
    pub unsafe fn copy_from_raw(
        event_base: *const c_char,
        event_id: i32,
        event_data: *const c_void,
        event_data_size: usize,
    ) -> Result<Self, EventCopyError> {
        if event_base.is_null() {
            return Err(EventCopyError::NullBase);
        }
        if event_data_size > DATA {
            return Err(EventCopyError::DataTooLarge(event_data_size));
        }
        if event_data_size != 0 && event_data.is_null() {
            return Err(EventCopyError::NullData);
        }

        let mut event = Self {
            base: [0; WIFI_EVENT_BASE_CAPACITY],
            base_len: 0,
            id: event_id,
            data: [0; DATA],
            data_len: event_data_size,
        };

        while event.base_len < WIFI_EVENT_BASE_CAPACITY {
            let byte = event_base.cast::<u8>().add(event.base_len).read();
            if byte == 0 {
                break;
            }
            event.base[event.base_len] = byte;
            event.base_len += 1;
        }
        if event.base_len == WIFI_EVENT_BASE_CAPACITY {
            return Err(EventCopyError::BaseTooLong);
        }

        if event_data_size != 0 {
            event.data[..event_data_size].copy_from_slice(slice::from_raw_parts(
                event_data.cast::<u8>(),
                event_data_size,
            ));
        }
        Ok(event)
    }
}

/// Bounded ownership bridge for the synchronous `_event_post` ABI.
pub struct WifiEventBridge<const N: usize, const DATA: usize> {
    channel: BoundedChannel<OwnedWifiEvent<DATA>, N>,
    rejected: AtomicUsize,
}

impl<const N: usize, const DATA: usize> WifiEventBridge<N, DATA> {
    pub const fn new() -> Self {
        Self {
            channel: BoundedChannel::new(),
            rejected: AtomicUsize::new(0),
        }
    }

    /// Copy and enqueue without honoring the synchronous timeout argument.
    ///
    /// # Safety
    /// The pointer requirements are the same as
    /// [`OwnedWifiEvent::copy_from_raw`].
    pub unsafe fn try_post_raw(
        &self,
        event_base: *const c_char,
        event_id: i32,
        event_data: *const c_void,
        event_data_size: usize,
    ) -> Result<(), EventCopyError> {
        let event = match OwnedWifiEvent::copy_from_raw(
            event_base,
            event_id,
            event_data,
            event_data_size,
        ) {
            Ok(event) => event,
            Err(error) => {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                return Err(error);
            }
        };
        self.channel.try_send(event).map_err(|_| {
            self.rejected.fetch_add(1, Ordering::Relaxed);
            EventCopyError::ChannelFull
        })
    }

    pub fn try_receive(&self) -> Option<OwnedWifiEvent<DATA>> {
        self.channel.try_receive()
    }

    pub fn receive(&self) -> Receive<'_, OwnedWifiEvent<DATA>, N> {
        self.channel.receive()
    }

    pub fn rejected(&self) -> usize {
        self.rejected.load(Ordering::Acquire)
    }
}

impl<const N: usize, const DATA: usize> Default for WifiEventBridge<N, DATA> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "riscv32")]
mod target {
    use core::{
        ffi::{c_char, c_void},
        mem,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use esp_wifi_sys_esp32s31::include::wifi_osi_funcs_t;

    use super::WifiEventBridge;

    type EventPostHandler = unsafe fn(usize, *const c_char, i32, *const c_void, usize) -> bool;

    static EVENT_BRIDGE_CONTEXT: AtomicUsize = AtomicUsize::new(0);
    static EVENT_BRIDGE_HANDLER: AtomicUsize = AtomicUsize::new(0);

    /// Route Wi-Fi events into a fixed-capacity owned Rust channel.
    ///
    /// This intentionally replaces delivery to the ESP event loop. The async
    /// consumer of `bridge` becomes responsible for application-facing event
    /// dispatch. Installation must be serialized with Wi-Fi init/deinit and
    /// the bridge must remain live while the table is registered.
    ///
    /// # Safety
    /// The table must not be registered or invoked concurrently with this
    /// update. Only one event bridge may be installed globally.
    pub unsafe fn patch_async_event_post<const N: usize, const DATA: usize>(
        table: &mut wifi_osi_funcs_t,
        bridge: &'static WifiEventBridge<N, DATA>,
    ) {
        EVENT_BRIDGE_CONTEXT.store(bridge as *const _ as usize, Ordering::Relaxed);
        EVENT_BRIDGE_HANDLER.store(
            event_post_thunk::<N, DATA> as EventPostHandler as usize,
            Ordering::Release,
        );
        table._event_post = Some(event_post);
    }

    unsafe fn event_post_thunk<const N: usize, const DATA: usize>(
        context: usize,
        event_base: *const c_char,
        event_id: i32,
        event_data: *const c_void,
        event_data_size: usize,
    ) -> bool {
        let bridge = &*(context as *const WifiEventBridge<N, DATA>);
        bridge
            .try_post_raw(event_base, event_id, event_data, event_data_size)
            .is_ok()
    }

    unsafe extern "C" fn event_post(
        event_base: *const c_char,
        event_id: i32,
        event_data: *mut c_void,
        event_data_size: usize,
        _ticks_to_wait: u32,
    ) -> i32 {
        let handler = EVENT_BRIDGE_HANDLER.load(Ordering::Acquire);
        if handler == 0 {
            return -1;
        }
        let handler = mem::transmute::<usize, EventPostHandler>(handler);
        let context = EVENT_BRIDGE_CONTEXT.load(Ordering::Relaxed);
        if handler(
            context,
            event_base,
            event_id,
            event_data.cast_const(),
            event_data_size,
        ) {
            0
        } else {
            -1
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub use target::patch_async_event_post;

#[cfg(test)]
mod tests {
    use core::{ffi::c_void, ptr};

    use super::{EventCopyError, WifiEventBridge};

    #[test]
    fn event_bridge_owns_base_and_payload() {
        let bridge = WifiEventBridge::<2, 8>::new();
        let mut payload = [1u8, 2, 3];
        unsafe {
            bridge
                .try_post_raw(
                    c"WIFI_EVENT".as_ptr(),
                    7,
                    payload.as_ptr().cast::<c_void>(),
                    payload.len(),
                )
                .unwrap();
        }
        payload.fill(9);
        let event = bridge.try_receive().unwrap();
        assert_eq!(event.base(), b"WIFI_EVENT");
        assert_eq!(event.id(), 7);
        assert_eq!(event.data(), &[1, 2, 3]);
    }

    #[test]
    fn oversize_and_null_payloads_are_rejected() {
        let bridge = WifiEventBridge::<1, 2>::new();
        assert_eq!(
            unsafe { bridge.try_post_raw(c"WIFI_EVENT".as_ptr(), 1, ptr::null(), 3) },
            Err(EventCopyError::DataTooLarge(3))
        );
        assert_eq!(
            unsafe { bridge.try_post_raw(c"WIFI_EVENT".as_ptr(), 1, ptr::null(), 1) },
            Err(EventCopyError::NullData)
        );
        assert_eq!(bridge.rejected(), 2);
    }
}
