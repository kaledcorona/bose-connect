//! The devices bluez already knows, so nobody has to type an address.
//!
//! `org.bluez` over the system bus, one `GetManagedObjects` call. Pairing
//! itself stays with `bluetoothctl` or the desktop: this only reads what is
//! there. Behind the `bluez` feature because it brings a D-Bus stack with it
//! and the rest of the crate has one dependency.

use std::io;

use zbus::blocking::{fdo::ObjectManagerProxy, Connection};
use zbus::zvariant::{OwnedValue, Value};

use super::Address;

/// A paired device, as bluez describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub address: Address,
    pub name: String,
    /// From `Modalias`, `bluetooth:v009Ep4066d0107`: the SIG company id and
    /// the product id, which is what `00 03` answers over RFCOMM.
    pub vendor: Option<u16>,
    pub product: Option<u16>,
    pub connected: bool,
}

/// Bose's Bluetooth SIG company identifier.
pub const BOSE: u16 = 0x009e;

impl Peer {
    pub fn is_bose(&self) -> bool {
        self.vendor == Some(BOSE)
    }
}

/// `bluetooth:v009Ep4066d0107` — vendor, product, device version, hex. The
/// `usb:` form appears on some stacks and carries the same three fields.
pub fn modalias(s: &str) -> Option<(u16, u16)> {
    let body = s.split_once(':')?.1;
    let (v, rest) = body.strip_prefix('v')?.split_once('p')?;
    // Fixed width: the `d` that starts the version field is also a hex digit.
    let p = rest.get(..4)?;
    Some((u16::from_str_radix(v, 16).ok()?, u16::from_str_radix(p, 16).ok()?))
}

fn string(v: Option<&OwnedValue>) -> Option<String> {
    match v.map(|v| v.downcast_ref::<Value<'_>>()) {
        Some(Ok(Value::Str(s))) => Some(s.to_string()),
        _ => None,
    }
}

fn flag(v: Option<&OwnedValue>) -> bool {
    matches!(v.map(|v| v.downcast_ref::<Value<'_>>()), Some(Ok(Value::Bool(true))))
}

fn bus(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("bluez: {e}"))
}

/// Every paired device, in bluez's order.
pub fn paired() -> io::Result<Vec<Peer>> {
    let conn = Connection::system().map_err(bus)?;
    let om = ObjectManagerProxy::builder(&conn)
        .destination("org.bluez")
        .map_err(bus)?
        .path("/")
        .map_err(bus)?
        .build()
        .map_err(bus)?;
    let objects = om.get_managed_objects().map_err(bus)?;
    let mut out = Vec::new();
    for (_, interfaces) in objects {
        let Some(dev) = interfaces.get("org.bluez.Device1") else { continue };
        if !flag(dev.get("Paired")) {
            continue;
        }
        let Some(address) = string(dev.get("Address")).and_then(|a| a.parse().ok()) else {
            continue;
        };
        let ids = string(dev.get("Modalias")).and_then(|m| modalias(&m));
        out.push(Peer {
            address,
            name: string(dev.get("Name")).or_else(|| string(dev.get("Alias"))).unwrap_or_default(),
            vendor: ids.map(|i| i.0),
            product: ids.map(|i| i.1),
            connected: flag(dev.get("Connected")),
        });
    }
    Ok(out)
}

/// The paired devices that are Bose's, which is what a picker lists.
pub fn bose() -> io::Result<Vec<Peer>> {
    Ok(paired()?.into_iter().filter(Peer::is_bose).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modalias_yields_the_ids_the_device_reports_over_rfcomm() {
        // `00 03` answers 40 66 01 on this device; bluez knows before connecting.
        assert_eq!(modalias("bluetooth:v009Ep4066d0107"), Some((0x009e, 0x4066)));
        assert_eq!(modalias("usb:v009Ep400Cd0007"), Some((0x009e, 0x400c)));
        assert_eq!(modalias("bluetooth:v009E"), None);
        assert_eq!(modalias("garbage"), None);
    }

    #[test]
    fn only_bose_devices_pass_the_picker() {
        let mk = |vendor| Peer {
            address: "AA:BB:CC:00:00:01".parse().unwrap(),
            name: String::new(),
            vendor,
            product: None,
            connected: false,
        };
        assert!(mk(Some(BOSE)).is_bose());
        assert!(!mk(Some(0x004c)).is_bose()); // Apple
        assert!(!mk(None).is_bose());
    }
}
