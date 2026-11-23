use std::{io, str::FromStr};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsbCameraIdentity {
    vendor: u16,
    product: u16,
    serial: String,
}

impl FromStr for UsbCameraIdentity {
    type Err = io::Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "Expected VID:PID:USB_SERIAL");
        let mut fields = input.splitn(3, ':');
        let vendor = parse_hex(fields.next().ok_or_else(invalid)?).ok_or_else(invalid)?;
        let product = parse_hex(fields.next().ok_or_else(invalid)?).ok_or_else(invalid)?;
        let serial = fields
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(invalid)?;
        Ok(Self {
            vendor,
            product,
            serial: serial.to_owned(),
        })
    }
}

fn parse_hex(value: &str) -> Option<u16> {
    u16::from_str_radix(value.strip_prefix("0x").unwrap_or(value), 16).ok()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn find_usb_device(identity: &UsbCameraIdentity) -> io::Result<nusb::DeviceInfo> {
    use nusb::MaybeFuture;

    let devices = nusb::list_devices().wait()?;
    unique_device(devices.filter(|device| {
        device.vendor_id() == identity.vendor
            && device.product_id() == identity.product
            && device.serial_number() == Some(identity.serial.as_str())
    }))
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn unique_device<T>(mut matches: impl Iterator<Item = T>) -> io::Result<T> {
    let first = matches.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "USB camera disconnected (no matching device)",
        )
    })?;
    if matches.next().is_some() {
        return Err(io::Error::other(
            "USB camera is ambiguous (multiple matching devices). Use rua camera-devices and pass --camera NAME=DEVICE",
        ));
    }
    Ok(first)
}

#[cfg(target_os = "linux")]
pub(crate) fn resolve(identity: &UsbCameraIdentity) -> io::Result<String> {
    use std::fs;
    use v4l::capability::Flags;

    let physical = find_usb_device(identity)?;
    let devices = fs::read_dir("/sys/class/video4linux")?;
    let index = linux_capture_indices(
        devices.filter_map(Result::ok).map(|entry| entry.path()),
        physical.sysfs_path(),
    )
    .filter(|index| {
        v4l::Device::new(*index)
            .and_then(|device| device.query_caps())
            .is_ok_and(|caps| caps.capabilities.contains(Flags::VIDEO_CAPTURE))
    })
    .min()
    .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Matching USB camera has no accessible video capture device",
        )
    })?;

    // WHY: Numeric selectors stop on capture loss. The publisher can then resolve USB identity again.
    Ok(index.to_string())
}

#[cfg(target_os = "linux")]
fn linux_capture_indices(
    nodes: impl Iterator<Item = std::path::PathBuf>,
    physical: &std::path::Path,
) -> impl Iterator<Item = usize> {
    nodes
        .filter(move |path| {
            std::fs::canonicalize(path.join("device"))
                .is_ok_and(|device| device.starts_with(physical))
        })
        .filter_map(|path| {
            path.file_name()?
                .to_str()?
                .strip_prefix("video")?
                .parse()
                .ok()
        })
}

#[cfg(target_os = "macos")]
pub(crate) fn resolve(identity: &UsbCameraIdentity) -> io::Result<String> {
    let usb_device = find_usb_device(identity)?;
    let capture_devices = cameras::list_cameras().map_err(io::Error::other)?;
    resolve_mac_capture_index(
        identity,
        usb_device.location_id(),
        capture_devices
            .iter()
            .map(|device| (device.misc(), device.index().as_string())),
    )
}

#[cfg(any(target_os = "macos", test))]
fn resolve_mac_capture_index(
    identity: &UsbCameraIdentity,
    location: u32,
    devices: impl Iterator<Item = (String, String)>,
) -> io::Result<String> {
    // WHY: Some USB capture IDs encode the location, vendor and product. Require all three or leave the camera unavailable.
    let expected_id = (u64::from(location) << 32)
        | (u64::from(identity.vendor) << 16)
        | u64::from(identity.product);
    let mut matches = devices.filter(|(id, _)| {
        id.strip_prefix("0x")
            .and_then(|hex| u64::from_str_radix(hex, 16).ok())
            == Some(expected_id)
    });
    let capture = matches.next()
        .ok_or_else(|| io::Error::other("Matching USB camera has no AVFoundation capture ID. Use rua camera-devices and pass --camera NAME=DEVICE"))?;
    if matches.next().is_some() {
        return Err(io::Error::other(
            "Matching USB camera has multiple AVFoundation capture IDs",
        ));
    }
    // WHY: The capture index forces a fresh USB match after a disconnect.
    Ok(capture.1)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn resolve(_identity: &UsbCameraIdentity) -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "USB camera matching is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_selection_requires_exactly_one_physical_match() {
        assert_eq!(unique_device(["wrist"].into_iter()).unwrap(), "wrist");
        assert_eq!(
            unique_device(std::iter::empty::<&str>())
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert!(
            unique_device(["wrist", "second wrist"].into_iter())
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
    }

    #[test]
    fn mac_capture_index_follows_the_matching_usb_location() {
        let identity: UsbCameraIdentity = "1234:abcd:TEAM_CAMERA".parse().unwrap();
        let capture_devices = [("0x147000001234abcd", "0"), ("0x146000001234abcd", "2")];
        let resolve = |location| {
            resolve_mac_capture_index(
                &identity,
                location,
                capture_devices
                    .into_iter()
                    .map(|(id, index)| (id.to_owned(), index.to_owned())),
            )
        };
        assert_eq!(resolve(0x1460_0000).unwrap(), "2");
        assert!(resolve(0x1480_0000).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_capture_nodes_follow_the_matching_usb_parent() {
        use std::{fs, os::unix::fs::symlink};

        let test_dir = tempfile::tempdir().unwrap();
        let wrist = test_dir.path().join("wrist");
        let laptop = test_dir.path().join("laptop");
        for physical_device in [&wrist, &laptop] {
            fs::create_dir_all(physical_device.join("interface")).unwrap();
        }
        let nodes =
            [("video0", &laptop), ("video2", &wrist), ("video3", &wrist)].map(|(name, device)| {
                let node = test_dir.path().join(name);
                fs::create_dir(&node).unwrap();
                symlink(device.join("interface"), node.join("device")).unwrap();
                node
            });
        assert_eq!(
            linux_capture_indices(nodes.into_iter(), &wrist).collect::<Vec<_>>(),
            [2, 3],
        );
    }
}
