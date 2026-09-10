//! Synthetic SAR input streams, independent of either calibration algorithm.
use open_radio_vendor_models_esp32s31::execution_model::{
    DeviceModel, DeviceModelDescriptor, DeviceModelInstance, Error, MemoryRange, Result,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Samples {
    Constant(u16),
    Alternating,
}

impl DeviceModel for Samples {
    fn descriptor(&self) -> DeviceModelDescriptor {
        DeviceModelDescriptor {
            id: "txdc-sar-samples".into(),
            kind: "synthetic-sar-stream".into(),
            range: MemoryRange {
                start: 0x2010_081c,
                length: 4,
            },
            configuration: BTreeMap::from([("profile".into(), format!("{self:?}"))]),
        }
    }

    fn instantiate(&self) -> Result<Box<dyn DeviceModelInstance>> {
        Ok(Box::new(Stream {
            profile: *self,
            index: 0,
        }))
    }
}

#[derive(Debug)]
struct Stream {
    profile: Samples,
    index: usize,
}

impl DeviceModelInstance for Stream {
    fn read(&mut self, address: u32, width: u8) -> Result<u32> {
        if address != 0x2010_081c || width != 32 {
            return Err(Error::Invalid {
                message: "unsupported SAR stream read".into(),
            });
        }
        // Repeating paired values exercise averaging, comparison direction,
        // minima and early scan termination. This is not an RF response model.
        let value = match self.profile {
            Samples::Constant(value) => value,
            Samples::Alternating => [100, 100, 130, 130, 90, 90, 80, 80][self.index],
        };
        self.index = (self.index + 1) % 8;
        Ok(u32::from(value) << 17)
    }

    fn write(&mut self, _: u32, _: u8, _: u32) -> Result<()> {
        Err(Error::Invalid {
            message: "SAR result is read-only".into(),
        })
    }
}
