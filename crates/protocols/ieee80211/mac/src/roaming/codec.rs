//! Private bounded cursors for the k/v codecs. Integer conversion and byte
//! order use the standard library at each field; this module owns only slicing.
/// Sequential field decoding keeps offsets local to the common byte reader.
pub(super) struct BodyReader<'a> {
    remaining: &'a [u8],
}
impl<'a> BodyReader<'a> {
    pub(super) const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }
    pub(super) fn take(&mut self, len: usize) -> Result<&'a [u8], super::WireError> {
        let value = self
            .remaining
            .get(..len)
            .ok_or(super::WireError::Truncated)?;
        self.remaining = &self.remaining[len..];
        Ok(value)
    }
    pub(super) fn array<const N: usize>(&mut self) -> Result<[u8; N], super::WireError> {
        Ok(self.take(N)?.try_into().expect("checked field width"))
    }
    pub(super) fn u8(&mut self) -> Result<u8, super::WireError> {
        let (value, remaining) = self
            .remaining
            .split_first()
            .ok_or(super::WireError::Truncated)?;
        self.remaining = remaining;
        Ok(*value)
    }
    pub(super) const fn remaining(&self) -> &'a [u8] {
        self.remaining
    }
}
/// Used only after validating every field and the complete encoded length.
pub(super) struct BodyWriter<'a> {
    remaining: &'a mut [u8],
}
impl<'a> BodyWriter<'a> {
    pub(super) fn new(bytes: &'a mut [u8]) -> Self {
        Self { remaining: bytes }
    }
    pub(super) fn put(&mut self, bytes: &[u8]) {
        let remaining = core::mem::take(&mut self.remaining);
        let (field, rest) = remaining.split_at_mut(bytes.len());
        field.copy_from_slice(bytes);
        self.remaining = rest;
    }
}
