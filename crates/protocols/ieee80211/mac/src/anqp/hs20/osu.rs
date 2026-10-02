//! Nested OSU and icon records. Binary content and URI execution stay external.
use super::*;
const ICON_FIXED_LEN: usize = 2 * core::mem::size_of::<u16>() + LANGUAGE_CODE_LEN + 2;
const PROVIDER_FIXED_LEN: usize = 3 * core::mem::size_of::<u16>() + 3;
const ICON_FILE_FIXED_LEN: usize = 2 + core::mem::size_of::<u16>();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IconMetadata<'a> {
    pub width: u16,
    pub height: u16,
    pub language: [u8; LANGUAGE_CODE_LEN],
    pub mime: &'a str,
    pub filename: &'a str,
}
impl IconMetadata<'_> {
    fn encoded_len(self) -> Result<usize, WireError> {
        if !self.language.is_ascii() || !self.mime.is_ascii() {
            return Err(WireError::InvalidText);
        }
        if self.mime.len() > u8::MAX as usize || self.filename.len() > u8::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        Ok(ICON_FIXED_LEN + self.mime.len() + self.filename.len())
    }
    fn write(self, fields: &mut BodyWriter<'_>) {
        fields.put(&self.width.to_le_bytes());
        fields.put(&self.height.to_le_bytes());
        fields.put(&self.language);
        fields.put(&[self.mime.len() as u8]);
        fields.put(self.mime.as_bytes());
        fields.put(&[self.filename.len() as u8]);
        fields.put(self.filename.as_bytes());
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Icons<'a>(&'a [u8]);
impl<'a> Icons<'a> {
    pub const EMPTY: Self = Self(&[]);
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        while !fields.remaining().is_empty() {
            read_icon(&mut fields)?;
        }
        Ok(Self(bytes))
    }
    pub fn iter(self) -> impl Iterator<Item = IconMetadata<'a>> + Clone {
        let mut fields = BodyReader::new(self.0);
        core::iter::from_fn(move || {
            if fields.remaining().is_empty() {
                None
            } else {
                Some(read_icon(&mut fields).expect("validated icons"))
            }
        })
    }
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }
    pub fn encode(icons: &[IconMetadata<'_>], bytes: &mut [u8]) -> Result<usize, WireError> {
        let mut length = 0usize;
        for icon in icons {
            length = length
                .checked_add(icon.encoded_len()?)
                .ok_or(WireError::ElementTooLong)?;
        }
        if length > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        let mut fields = BodyWriter::new(output(bytes, length)?);
        for icon in icons {
            icon.write(&mut fields);
        }
        Ok(length)
    }
}
fn read_icon<'a>(fields: &mut BodyReader<'a>) -> Result<IconMetadata<'a>, WireError> {
    let icon = IconMetadata {
        width: u16::from_le_bytes(fields.array()?),
        height: u16::from_le_bytes(fields.array()?),
        language: fields.array()?,
        mime: utf8(short_bytes(fields)?)?,
        filename: utf8(short_bytes(fields)?)?,
    };
    icon.encoded_len()?;
    Ok(icon)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OsuProvider<'a> {
    pub names: Names<'a>,
    pub server_uri: &'a [u8],
    pub methods: &'a [u8],
    pub icons: Icons<'a>,
    pub nai: &'a str,
    pub service_description: Names<'a>,
}
impl OsuProvider<'_> {
    fn body_len(self) -> Result<usize, WireError> {
        for length in [self.server_uri.len(), self.methods.len(), self.nai.len()] {
            if length > u8::MAX as usize {
                return Err(WireError::ElementTooLong);
            }
        }
        for length in [
            self.names.as_bytes().len(),
            self.icons.as_bytes().len(),
            self.service_description.as_bytes().len(),
        ] {
            if length > u16::MAX as usize {
                return Err(WireError::ElementTooLong);
            }
        }
        let length = PROVIDER_FIXED_LEN
            + self.names.as_bytes().len()
            + self.server_uri.len()
            + self.methods.len()
            + self.icons.as_bytes().len()
            + self.nai.len()
            + self.service_description.as_bytes().len();
        if length > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        Ok(length)
    }
    fn write(self, fields: &mut BodyWriter<'_>) {
        fields.put(&(self.body_len().expect("validated OSU provider length") as u16).to_le_bytes());
        fields.put(&(self.names.as_bytes().len() as u16).to_le_bytes());
        fields.put(self.names.as_bytes());
        fields.put(&[self.server_uri.len() as u8]);
        fields.put(self.server_uri);
        fields.put(&[self.methods.len() as u8]);
        fields.put(self.methods);
        fields.put(&(self.icons.as_bytes().len() as u16).to_le_bytes());
        fields.put(self.icons.as_bytes());
        fields.put(&[self.nai.len() as u8]);
        fields.put(self.nai.as_bytes());
        fields.put(&(self.service_description.as_bytes().len() as u16).to_le_bytes());
        fields.put(self.service_description.as_bytes());
    }
}
fn read_provider<'a>(fields: &mut BodyReader<'a>) -> Result<OsuProvider<'a>, WireError> {
    let mut record = BodyReader::new(long_bytes(fields)?);
    let provider = OsuProvider {
        names: Names::parse(long_bytes(&mut record)?)?,
        server_uri: short_bytes(&mut record)?,
        methods: short_bytes(&mut record)?,
        icons: Icons::parse(long_bytes(&mut record)?)?,
        nai: utf8(short_bytes(&mut record)?)?,
        service_description: Names::parse(long_bytes(&mut record)?)?,
    };
    end(&record)?;
    Ok(provider)
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OsuProviders<'a> {
    pub ssid: &'a [u8],
    count: u8,
    records: &'a [u8],
}
impl<'a> OsuProviders<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        let ssid_bytes = short_bytes(&mut fields)?;
        ssid(ssid_bytes)?;
        let count = fields.u8()?;
        let records = fields.remaining();
        for _ in 0..count {
            read_provider(&mut fields)?;
        }
        end(&fields)?;
        Ok(Self {
            ssid: ssid_bytes,
            count,
            records,
        })
    }
    pub const fn count(self) -> u8 {
        self.count
    }
    pub fn iter(self) -> impl Iterator<Item = OsuProvider<'a>> + Clone {
        let mut rest = self.records;
        (0..self.count).map(move |_| {
            let mut fields = BodyReader::new(rest);
            let value = read_provider(&mut fields).expect("validated OSU providers");
            rest = fields.remaining();
            value
        })
    }
    pub fn encode(
        ssid_bytes: &[u8],
        providers: &[OsuProvider<'_>],
        bytes: &mut [u8],
    ) -> Result<usize, WireError> {
        ssid(ssid_bytes)?;
        let count = u8::try_from(providers.len()).map_err(|_| WireError::ElementTooLong)?;
        let mut length = 2 + ssid_bytes.len();
        for provider in providers {
            length = length
                .checked_add(core::mem::size_of::<u16>() + provider.body_len()?)
                .ok_or(WireError::ElementTooLong)?;
        }
        if length > u16::MAX as usize - VENDOR_HEADER_LEN {
            return Err(WireError::ElementTooLong);
        }
        let mut fields = BodyWriter::new(output(bytes, length)?);
        fields.put(&[ssid_bytes.len() as u8]);
        fields.put(ssid_bytes);
        fields.put(&[count]);
        for provider in providers {
            provider.write(&mut fields);
        }
        Ok(length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IconStatus(pub u8);
impl IconStatus {
    pub const SUCCESS: Self = Self(0);
    pub const FILE_NOT_FOUND: Self = Self(1);
    pub const UNSPECIFIED_ERROR: Self = Self(2);
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IconBinaryFile<'a> {
    pub status: IconStatus,
    pub mime: &'a str,
    pub data: &'a [u8],
}
impl<'a> IconBinaryFile<'a> {
    fn encoded_len(self) -> Result<usize, WireError> {
        if !self.mime.is_ascii() {
            return Err(WireError::InvalidText);
        }
        if self.mime.len() > u8::MAX as usize || self.data.len() > u16::MAX as usize {
            return Err(WireError::ElementTooLong);
        }
        if matches!(
            self.status,
            IconStatus::FILE_NOT_FOUND | IconStatus::UNSPECIFIED_ERROR
        ) && (!self.mime.is_empty() || !self.data.is_empty())
        {
            return Err(WireError::InvalidValue);
        }
        let length = ICON_FILE_FIXED_LEN + self.mime.len() + self.data.len();
        if length > u16::MAX as usize - VENDOR_HEADER_LEN {
            return Err(WireError::ElementTooLong);
        }
        Ok(length)
    }
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(bytes);
        let value = Self {
            status: IconStatus(fields.u8()?),
            mime: utf8(short_bytes(&mut fields)?)?,
            data: long_bytes(&mut fields)?,
        };
        end(&fields)?;
        value.encoded_len()?;
        Ok(value)
    }
    pub fn encode(self, bytes: &mut [u8]) -> Result<usize, WireError> {
        let length = self.encoded_len()?;
        let mut fields = BodyWriter::new(output(bytes, length)?);
        fields.put(&[self.status.0, self.mime.len() as u8]);
        fields.put(self.mime.as_bytes());
        fields.put(&(self.data.len() as u16).to_le_bytes());
        fields.put(self.data);
        Ok(length)
    }
}
