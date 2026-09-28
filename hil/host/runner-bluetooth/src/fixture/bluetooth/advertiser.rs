//! A BlueZ advertisement from the powered adapter of an [`super::att::Owner`].
//!
//! The fixture exports one `org.bluez.LEAdvertisement1` object and registers
//! it with the adapter's advertising manager. A broadcast advertisement with a
//! local name is scannable: BlueZ carries the flags and the manufacturer data
//! in the advertising PDU and the name in the scan response. The registration
//! belongs to the fixture's own system bus connection and ends with it.
use super::model::Adapter;
use crate::Result;
use std::{collections::HashMap, time::Duration};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedObjectPath, Value},
};

const PATH: &str = "/org/open_radio/hil/advertisement";
/// Manufacturer identifier reserved for testing (Core Supplement A 1.4).
const TEST_COMPANY: u16 = 0xffff;

struct Advertisement {
    name: String,
    marker: Vec<u8>,
}

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl Advertisement {
    fn release(&self) {}

    #[zbus(property)]
    fn r#type(&self) -> String {
        "broadcast".into()
    }

    #[zbus(property)]
    fn local_name(&self) -> String {
        self.name.clone()
    }

    #[zbus(property)]
    fn manufacturer_data(&self) -> HashMap<u16, Value<'static>> {
        HashMap::from([(TEST_COMPANY, Value::from(self.marker.clone()))])
    }
}

/// One registered advertisement; unregistering is part of the owner's cleanup.
pub struct Advertiser {
    bus: Connection,
    adapter: String,
    registered: bool,
}

impl Advertiser {
    /// Advertise `marker` as manufacturer data and `name` in the scan
    /// response.
    pub fn start(adapter: Adapter, name: &str, marker: &[u8]) -> Result<Self> {
        let bus = zbus::blocking::connection::Builder::system()?
            .method_timeout(Duration::from_secs(5))
            .build()?;
        bus.object_server().at(
            PATH,
            Advertisement {
                name: name.into(),
                marker: marker.to_vec(),
            },
        )?;
        let mut advertiser = Self {
            bus,
            adapter: format!("/org/bluez/{adapter}"),
            registered: false,
        };
        let options: HashMap<&str, Value> = HashMap::new();
        advertiser.manager()?.call::<_, _, ()>(
            "RegisterAdvertisement",
            &(OwnedObjectPath::try_from(PATH)?, options),
        )?;
        advertiser.registered = true;
        Ok(advertiser)
    }

    fn manager(&self) -> Result<Proxy<'_>> {
        Ok(Proxy::new(
            &self.bus,
            "org.bluez",
            self.adapter.as_str(),
            "org.bluez.LEAdvertisingManager1",
        )?)
    }

    /// Advertising instances BlueZ runs on the adapter.
    pub fn active_instances(&self) -> Result<u8> {
        Ok(self.manager()?.get_property::<u8>("ActiveInstances")?)
    }

    pub fn stop(&mut self) -> Result<()> {
        if !self.registered {
            return Ok(());
        }
        self.registered = false;
        self.manager()?.call::<_, _, ()>(
            "UnregisterAdvertisement",
            &(OwnedObjectPath::try_from(PATH)?,),
        )?;
        Ok(())
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        let _ = oer_process::cleanup(|| self.stop());
    }
}
