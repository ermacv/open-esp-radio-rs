//! Named peripheral inputs for declarative current-PHY comparisons.
//!
//! These models supply finite register readiness and synthetic measurement
//! streams. They do not implement a calibration decision or assert RF quality.

use crate::{
    execution_model::{
        DeviceModel, DeviceModelDescriptor, DeviceModelInstance, DeviceModelRegistry, Error,
        MemoryRange, Result,
    },
    phy_i2c::RegisterBank,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug)]
pub enum SarSamples {
    Constant(u16),
    Alternating,
}

impl DeviceModel for SarSamples {
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
        crate::execution_model::admission::mechanism("phy-calibration");
        Ok(Box::new(SarStream {
            profile: *self,
            index: 0,
        }))
    }
}

#[derive(Debug)]
struct SarStream {
    profile: SarSamples,
    index: usize,
}

impl DeviceModelInstance for SarStream {
    fn read(&mut self, address: u32, width: u8) -> Result<u32> {
        if address != 0x2010_081c || width != 32 {
            return Err(Error::Invalid {
                message: "unsupported SAR stream read".into(),
            });
        }
        let value = match self.profile {
            SarSamples::Constant(value) => value,
            SarSamples::Alternating => [100, 100, 130, 130, 90, 90, 80, 80][self.index],
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

pub fn register(registry: &mut DeviceModelRegistry) {
    for (name, samples) in [
        ("constant-0", SarSamples::Constant(0)),
        ("constant-123", SarSamples::Constant(123)),
        ("constant-8191", SarSamples::Constant(8191)),
        ("alternating", SarSamples::Alternating),
    ] {
        registry
            .register(format!("esp32s31.txdcal.sar.{name}"), Arc::new(samples))
            .expect("static SAR model IDs are unique");
    }
    for fill in [0x5a, 0xa5] {
        for busy in [0, 2] {
            registry
                .register(
                    format!("esp32s31.rxcal.dcode-i2c.fill-{fill:02x}.busy-{busy}"),
                    Arc::new(RegisterBank {
                        bbpll_control: None,
                        registers: BTreeMap::from([
                            ((0x62, 4), fill),
                            ((0x62, 19), fill),
                            ((0x62, 20), fill),
                        ]),
                        reads: BTreeMap::from([
                            ((0x62, 17), vec![0xc0, 0xdf, 0xe0, 0xff]),
                            ((0x62, 18), vec![0xff, 0xe0, 0xdf, 0xc0]),
                        ]),
                        busy_reads: busy,
                    }),
                )
                .expect("static D-code model IDs are unique");
            registry
                .register(
                    format!("esp32s31.rxcal.gain-i2c.fill-{fill:02x}.busy-{busy}"),
                    Arc::new(RegisterBank {
                        bbpll_control: None,
                        registers: BTreeMap::from([((0x67, 3), fill)]),
                        reads: BTreeMap::new(),
                        busy_reads: busy,
                    }),
                )
                .expect("static RX-gain model IDs are unique");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarative_registry_exposes_only_explicit_finite_inputs() {
        let mut registry = DeviceModelRegistry::default();
        register(&mut registry);

        assert!(registry.resolve("esp32s31.txdcal.sar.alternating").is_ok());
        assert!(
            registry
                .resolve("esp32s31.rxcal.dcode-i2c.fill-5a.busy-2")
                .is_ok()
        );
        assert!(
            registry
                .resolve("esp32s31.rxcal.infer-from-address")
                .is_err()
        );
    }
}
