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
    let output = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-a", "-r", "-c", "IOUSBHostDevice", "-d", "1"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "Could not read the macOS USB device list: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let devices = read_mac_usb_devices(&output.stdout)?;
    let location = mac_usb_location(identity, &devices)?;
    let capture_devices = cameras::list_cameras().map_err(io::Error::other)?;
    resolve_mac_capture_index(
        identity,
        location,
        capture_devices
            .iter()
            .map(|device| (device.misc(), device.index().as_string())),
    )
}

#[cfg(any(target_os = "macos", test))]
#[derive(serde::Deserialize)]
struct MacUsbDevice {
    #[serde(rename = "idVendor")]
    vendor: Option<u16>,
    #[serde(rename = "idProduct")]
    product: Option<u16>,
    #[serde(rename = "USB Serial Number")]
    serial: Option<String>,
    #[serde(rename = "locationID")]
    location: Option<u32>,
}

#[cfg(any(target_os = "macos", test))]
fn read_mac_usb_devices(archive: &[u8]) -> io::Result<Vec<MacUsbDevice>> {
    // WHY: ioreg emits no archive when a rooted search finds no matching class.
    if archive.is_empty() {
        return Ok(Vec::new());
    }
    plist::from_bytes(archive).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Could not decode the macOS USB device list: {error}"),
        )
    })
}

#[cfg(any(target_os = "macos", test))]
fn mac_usb_location(identity: &UsbCameraIdentity, devices: &[MacUsbDevice]) -> io::Result<u32> {
    unique_device(devices.iter().filter(|device| {
        device.vendor == Some(identity.vendor)
            && device.product == Some(identity.product)
            && device.serial.as_deref() == Some(identity.serial.as_str())
    }))?
    .location
    .ok_or_else(|| io::Error::other("Matching USB camera has no macOS location ID"))
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
    fn mac_camera_uses_usb_identity_and_rejects_duplicate_physical_devices() {
        let expected: UsbCameraIdentity = "1234:abcd:TEAM_CAMERA".parse().unwrap();
        let usb_camera = |serial: &str, location: u32| {
            format!(
                "<dict>
                    <key>idVendor</key><integer>4660</integer>
                    <key>idProduct</key><integer>43981</integer>
                    <key>USB Serial Number</key><string>{serial}</string>
                    <key>locationID</key><integer>{location}</integer>
                    <key>IOObjectClass</key><string>IOUSBHostDevice</string>
                </dict>"
            )
        };
        let mut cameras = vec![
            usb_camera("TEAM_CAMERA", 0x1460_0000),
            usb_camera("LAPTOP_CAMERA", 0x1470_0000),
        ];
        let capture_devices = [("0x147000001234abcd", "0"), ("0x146000001234abcd", "2")];
        let resolve = |cameras: &[String]| {
            let archive = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
                <plist version="1.0"><array>{}</array></plist>"#,
                cameras.concat()
            );
            let devices = read_mac_usb_devices(archive.as_bytes()).unwrap();
            let location = mac_usb_location(&expected, &devices)?;
            resolve_mac_capture_index(
                &expected,
                location,
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
        assert_eq!(
            mac_usb_location(&expected, &read_mac_usb_devices(b"").unwrap())
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        cameras.push(usb_camera("TEAM_CAMERA", 0x1480_0000));
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
