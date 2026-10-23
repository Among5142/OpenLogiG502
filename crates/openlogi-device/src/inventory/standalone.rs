//! Discovery of standalone raw-HID devices.

use std::collections::{HashMap, HashSet};

use openlogi_core::device::{DeviceKind, RawDeviceAddress, StandaloneDevice};
use openlogi_device_registry::headset::{GAMING_HEADSET_DRIVER_ID, find_gaming_headset};
use openlogi_device_registry::litra::find_litra;

use super::InventoryError;
use crate::backend::HidBackend;
use crate::write::litra_capabilities;

/// Enumerate recognized standalone devices without probing them as HID++.
///
/// The returned descriptors are intentionally separate from receiver
/// inventories. A raw device has no HID++ pairing slot and must be routed by
/// its full HID identity tuple.
pub async fn enumerate_standalone(
    backend: &dyn HidBackend,
) -> Result<Vec<StandaloneDevice>, InventoryError> {
    let devices = backend.enumerate().await?;
    let devices: Vec<_> = devices
        .into_iter()
        .filter_map(|device| standalone_device(&device))
        .collect();
    validate_no_ambiguous_nodes(&devices)?;
    Ok(devices)
}

/// Classifies one enumerated HID node against every standalone driver family
/// OpenLogi recognizes, in precedence order. `None` when no family claims it.
fn standalone_device(device: &crate::backend::NodeInfo) -> Option<StandaloneDevice> {
    if let Some(descriptor) = find_litra(
        device.vendor_id,
        device.product_id,
        device.usage_page,
        device.usage_id,
    ) {
        return Some(StandaloneDevice {
            address: RawDeviceAddress {
                vendor_id: device.vendor_id,
                product_id: device.product_id,
                usage_page: device.usage_page,
                usage_id: device.usage_id,
                identity: device.identity(),
            },
            display_name: device.name.clone(),
            manufacturer: device.manufacturer.clone(),
            serial_number: device.serial_number.clone(),
            unit_id: [0; 4],
            kind: DeviceKind::Light,
            online: true,
            capabilities: None,
            light_capabilities: Some(litra_capabilities(descriptor.model)),
            driver_id: descriptor.driver_id.to_owned(),
            registry_model_id: Some(descriptor.registry_model_id.to_owned()),
        });
    }

    if let Some(descriptor) = find_gaming_headset(
        device.vendor_id,
        device.product_id,
        device.usage_page,
        device.usage_id,
    ) {
        return Some(StandaloneDevice {
            address: RawDeviceAddress {
                vendor_id: device.vendor_id,
                product_id: device.product_id,
                usage_page: device.usage_page,
                usage_id: device.usage_id,
                identity: device.identity(),
            },
            display_name: descriptor.name.to_owned(),
            manufacturer: device.manufacturer.clone(),
            serial_number: device.serial_number.clone(),
            unit_id: [0; 4],
            kind: DeviceKind::Headset,
            online: true,
            capabilities: None,
            light_capabilities: None,
            driver_id: GAMING_HEADSET_DRIVER_ID.to_owned(),
            registry_model_id: Some(descriptor.registry_model_id.to_owned()),
        });
    }

    None
}

/// Reject multiple nodes that the route cannot distinguish safely: two
/// answering to the same VID/PID/usage tuple *and* the same identity.
///
/// Identical lights are told apart by serial number when they report one.
/// When they do not (some Litra Glows), each is still a distinct OS node, so
/// its `id:…` identity names it for as long as it stays plugged in where it
/// is; on Windows that path follows the USB port, so it survives a replug
/// into the same port. Such a light's settings stay session-only either way
/// (`PhysicalDeviceKey::is_transient`), so an identity that does move after
/// a replug leaves nothing saved against the wrong light. Only nodes the
/// route would match together are ambiguous.
fn validate_no_ambiguous_nodes(devices: &[StandaloneDevice]) -> Result<(), InventoryError> {
    let mut groups: HashMap<(u16, u16, u16, u16), Vec<&StandaloneDevice>> = HashMap::new();
    for device in devices {
        let address = &device.address;
        groups
            .entry((
                address.vendor_id,
                address.product_id,
                address.usage_page,
                address.usage_id,
            ))
            .or_default()
            .push(device);
    }
    if groups.values().any(|group| {
        let identities: HashSet<&str> = group
            .iter()
            .map(|device| device.address.identity.as_str())
            .collect();
        identities.len() != group.len()
    }) {
        return Err(InventoryError::AmbiguousRawDevice);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use openlogi_core::device::{DeviceKind, RawDeviceAddress, StandaloneDevice};

    use crate::write::matches_litra;

    use super::{InventoryError, validate_no_ambiguous_nodes};

    #[test]
    fn glow_fixture_matches_standalone_driver() {
        assert!(matches_litra(0x046d, 0xc900, 0xff43, 0x0202));
    }

    fn raw(identity: &str) -> StandaloneDevice {
        StandaloneDevice {
            address: RawDeviceAddress {
                vendor_id: 0x046d,
                product_id: 0xc900,
                usage_page: 0xff43,
                usage_id: 0x0202,
                identity: identity.into(),
            },
            display_name: "Litra Glow".into(),
            manufacturer: Some("Logi".into()),
            serial_number: identity.strip_prefix("serial:").map(str::to_string),
            unit_id: [0; 4],
            kind: DeviceKind::Light,
            online: true,
            capabilities: None,
            light_capabilities: None,
            driver_id: "litra".into(),
            registry_model_id: None,
        }
    }

    #[test]
    fn two_lights_without_serials_are_told_apart_by_their_nodes() {
        let devices = vec![raw("id:port-1"), raw("id:port-2")];
        let validation = validate_no_ambiguous_nodes(&devices);
        assert!(
            validation.is_ok(),
            "distinct OS nodes stay distinguishable: {validation:?}"
        );
    }

    #[test]
    fn nodes_the_route_cannot_tell_apart_are_rejected() {
        for identity in ["id:same", "serial:same"] {
            let devices = vec![raw(identity), raw(identity)];
            assert!(matches!(
                validate_no_ambiguous_nodes(&devices),
                Err(InventoryError::AmbiguousRawDevice)
            ));
        }
    }

    #[test]
    fn distinct_serials_can_share_the_same_hid_tuple() {
        let devices = vec![raw("serial:one"), raw("serial:two")];
        let validation = validate_no_ambiguous_nodes(&devices);
        assert!(
            validation.is_ok(),
            "serial-backed nodes stay distinguishable: {validation:?}"
        );
    }
}
