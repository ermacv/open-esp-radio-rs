/// Bounded inline storage used by the copying UDP API.
///
/// For externally placed storage, instantiate the engine with a pool lease
/// implementing `AsRef<[u8]>` and use `enqueue_udp_owned` instead.
#[derive(Debug)]
pub struct InlinePayload<const CAPACITY: usize> {
    bytes: [u8; CAPACITY],
    length: usize,
}

impl<const CAPACITY: usize> InlinePayload<CAPACITY> {
    pub fn copy_from_slice(payload: &[u8]) -> Option<Self> {
        if payload.len() > CAPACITY {
            return None;
        }
        let mut bytes = [0; CAPACITY];
        bytes[..payload.len()].copy_from_slice(payload);
        Some(Self {
            bytes,
            length: payload.len(),
        })
    }
}

impl<const CAPACITY: usize> AsRef<[u8]> for InlinePayload<CAPACITY> {
    fn as_ref(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}
