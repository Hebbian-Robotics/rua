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

    let physical = unique_device(
        fs::read_dir("/sys/bus/usb/devices")?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| linux_usb_match(path, identity))
            .filter_map(|path| fs::canonicalize(path).ok()),
    )?;
    let devices = fs::read_dir("/sys/class/video4linux")?;
    let index = linux_capture_indices(
        devices.filter_map(Result::ok).map(|entry| entry.path()),
        &physical,
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
fn linux_usb_match(path: &std::path::Path, expected: &UsbCameraIdentity) -> bool {
    use std::fs;
    let hex_field = |name| {
        fs::read_to_string(path.join(name))
            .ok()
            .and_then(|value| parse_hex(value.trim()))
    };
    hex_field("idVendor") == Some(expected.vendor)
        && hex_field("idProduct") == Some(expected.product)
        && fs::read_to_string(path.join("serial"))
            .is_ok_and(|serial| serial.trim() == expected.serial)
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
    let output = std::process::Command::new("/usr/sbin/system_profiler")
        .args(["-timeout", "5", "-json", "SPUSBDataType"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("Could not read the macOS USB device list"));
    }
    let profile: MacUsbProfile = serde_json::from_slice(&output.stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let devices = cameras::list_cameras().map_err(io::Error::other)?;
    resolve_mac_camera(
        identity,
        &profile,
        devices
            .iter()
            .map(|device| (device.misc(), device.index().as_string())),
    )
}

#[cfg(any(target_os = "macos", test))]
#[derive(serde::Deserialize)]
struct MacUsbProfile {
    #[serde(rename = "SPUSBDataType")]
    devices: Vec<MacUsbDevice>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(serde::Deserialize)]
struct MacUsbDevice {
    vendor_id: Option<String>,
    product_id: Option<String>,
    serial_num: Option<String>,
    location_id: Option<String>,
    #[serde(rename = "_items", default)]
    children: Vec<MacUsbDevice>,
}

#[cfg(any(target_os = "macos", test))]
impl MacUsbDevice {
    fn collect_matches<'a>(&'a self, identity: &UsbCameraIdentity, matches: &mut Vec<&'a Self>) {
        let number = |field: Option<&str>| {
            field
                .and_then(|field| field.split_whitespace().next())
                .and_then(parse_hex)
        };
        if number(self.vendor_id.as_deref()) == Some(identity.vendor)
            && number(self.product_id.as_deref()) == Some(identity.product)
            && self.serial_num.as_deref() == Some(&identity.serial)
        {
            matches.push(self);
        }
        for child in &self.children {
            child.collect_matches(identity, matches);
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn resolve_mac_camera(
    identity: &UsbCameraIdentity,
    profile: &MacUsbProfile,
    devices: impl Iterator<Item = (String, String)>,
) -> io::Result<String> {
    let mut matches = Vec::new();
    for device in &profile.devices {
        device.collect_matches(identity, &mut matches);
    }
    let usb_device = unique_device(matches.into_iter())?;
    let location = usb_device
        .location_id
        .as_deref()
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.strip_prefix("0x"))
        .and_then(|value| u32::from_str_radix(value, 16).ok())
        .ok_or_else(|| io::Error::other("Matching USB camera has no macOS location ID"))?;
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
    fn mac_camera_uses_usb_identity_and_rejects_duplicate_physical_devices() {
        let expected: UsbCameraIdentity = "1234:abcd:TEAM_CAMERA".parse().unwrap();
        let usb_camera = |serial: &str, location: &str| {
            serde_json::json!({
                "vendor_id": "0x1234 (Example)",
                "product_id": "0xabcd",
                "serial_num": serial,
                "location_id": location,
            })
        };
        let mut cameras = vec![
            usb_camera("TEAM_CAMERA", "0x14600000 / 4"),
            usb_camera("LAPTOP_CAMERA", "0x14700000 / 3"),
        ];
        let profile = |cameras: &[serde_json::Value]| {
            serde_json::from_value::<MacUsbProfile>(serde_json::json!({
                "SPUSBDataType": [{"_items": cameras}]
            }))
            .unwrap()
        };
        let capture_devices = [("0x147000001234abcd", "0"), ("0x146000001234abcd", "2")];
        let resolve = |cameras: &[serde_json::Value]| {
            resolve_mac_camera(
                &expected,
                &profile(cameras),
                capture_devices
                    .into_iter()
                    .map(|(id, index)| (id.to_owned(), index.to_owned())),
            )
        };
        assert_eq!(resolve(&cameras).unwrap(), "2");
        assert_eq!(
            resolve(&cameras[1..]).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        cameras.push(usb_camera("TEAM_CAMERA", "0x14800000 / 5"));
        assert!(
            resolve(&cameras)
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_capture_nodes_follow_the_matching_usb_parent() {
        use std::{fs, os::unix::fs::symlink};

        let test_dir = tempfile::tempdir().unwrap();
        let wrist = test_dir.path().join("wrist");
        let laptop = test_dir.path().join("laptop");
        for (path, serial) in [(&wrist, "TEAM_CAMERA"), (&laptop, "LAPTOP_CAMERA")] {
            fs::create_dir_all(path.join("interface")).unwrap();
            fs::write(path.join("idVendor"), "1234\n").unwrap();
            fs::write(path.join("idProduct"), "abcd\n").unwrap();
            fs::write(path.join("serial"), format!("{serial}\n")).unwrap();
        }
        let expected = "1234:abcd:TEAM_CAMERA".parse().unwrap();
        let physical = unique_device(
            [wrist.as_path(), laptop.as_path()]
                .into_iter()
                .filter(|path| linux_usb_match(path, &expected)),
        )
        .unwrap();
        let nodes =
            [("video0", &laptop), ("video2", &wrist), ("video3", &wrist)].map(|(name, device)| {
                let node = test_dir.path().join(name);
                fs::create_dir(&node).unwrap();
                symlink(device.join("interface"), node.join("device")).unwrap();
                node
            });
        assert_eq!(
            linux_capture_indices(nodes.into_iter(), physical).collect::<Vec<_>>(),
            [2, 3],
        );
    }
}
