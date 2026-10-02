//! Private bounded cursors shared by variable-length MAC formats.
//! Byte order stays explicit at each protocol field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Truncated;

/// Sequential field decoding keeps offsets local to the common byte reader.
#[derive(Clone)]
pub(crate) struct BodyReader<'a> {
    remaining: &'a [u8],
}
impl<'a> BodyReader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }
    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8], Truncated> {
        let value = self.remaining.get(..len).ok_or(Truncated)?;
        self.remaining = &self.remaining[len..];
        Ok(value)
    }
    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], Truncated> {
        Ok(self.take(N)?.try_into().expect("checked field width"))
    }
    pub(crate) fn u8(&mut self) -> Result<u8, Truncated> {
        let (value, remaining) = self.remaining.split_first().ok_or(Truncated)?;
        self.remaining = remaining;
        Ok(*value)
    }
    pub(crate) const fn remaining(&self) -> &'a [u8] {
        self.remaining
    }
}
/// Used only after validating every field and the complete encoded length.
pub(crate) struct BodyWriter<'a> {
    remaining: &'a mut [u8],
}
impl<'a> BodyWriter<'a> {
    pub(crate) fn new(bytes: &'a mut [u8]) -> Self {
        Self { remaining: bytes }
    }
    pub(crate) fn put(&mut self, bytes: &[u8]) {
        let remaining = core::mem::take(&mut self.remaining);
        let (field, rest) = remaining.split_at_mut(bytes.len());
        field.copy_from_slice(bytes);
        self.remaining = rest;
    }
}
