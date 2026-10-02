use super::*;
use oer_ieee80211_mac::management::elements::MAX_ENCODED_ELEMENT_LEN;

pub(crate) struct Bytes<const N: usize> {
    storage: [u8; N],
    len: usize,
}
impl<const N: usize> Bytes<N> {
    pub(crate) const fn empty() -> Self {
        Self {
            storage: [0; N],
            len: 0,
        }
    }
    pub(crate) fn copy(bytes: &[u8]) -> Result<Self, Error> {
        let mut value = Self::empty();
        value.append(bytes)?;
        Ok(value)
    }
    pub(crate) fn append(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let required = self.len.checked_add(bytes.len()).ok_or(Error::Capacity {
            required: usize::MAX,
            capacity: N,
        })?;
        if required > N {
            return Err(Error::Capacity {
                required,
                capacity: N,
            });
        }
        self.storage[self.len..required].copy_from_slice(bytes);
        self.len = required;
        Ok(())
    }
    pub(crate) fn frame(frame: gas::Frame<'_>) -> Result<Self, Error> {
        let required = frame.encoded_len()?;
        if required > N {
            return Err(Error::Capacity {
                required,
                capacity: N,
            });
        }
        let mut value = Self::empty();
        value.len = frame.encode(&mut value.storage)?;
        Ok(value)
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.storage[..self.len]
    }
    pub(crate) fn clear(&mut self) {
        self.len = 0;
    }
}

pub(crate) struct OwnedAdvertisement(Bytes<MAX_ENCODED_ELEMENT_LEN>);
impl OwnedAdvertisement {
    pub(crate) fn new(value: gas::AdvertisementProtocol<'_>) -> Result<Self, Error> {
        let mut bytes = Bytes::empty();
        bytes.len = value.encode(&mut bytes.storage)?;
        Ok(Self(bytes))
    }
    pub(crate) fn get(&self) -> gas::AdvertisementProtocol<'_> {
        gas::AdvertisementProtocol::parse(self.0.bytes()).expect("owned validated advertisement")
    }
}
