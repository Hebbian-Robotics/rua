# Windows with WSL2

RUA's prebuilt package targets Linux x86-64 with glibc 2.39 or newer. On an
x86-64 Windows PC, use Ubuntu 24.04 under WSL2 and run the normal
[installation and agent commands](getting-started.md) inside Ubuntu. Native
Windows and WSL hardware operation are not validated for RUA yet.

For a USB arm, follow [Microsoft's WSL USB guide](https://learn.microsoft.com/en-us/windows/wsl/connect-usb).
Installing usbipd and sharing a device require administrator access on Windows;
attaching it does not. Attach again after unplugging or restarting WSL, or use
usbipd's auto-attach functionality. An attached device is unavailable to Windows.

Before starting RUA:

- Confirm the arm appears inside Ubuntu and your user can access its serial port.
  USB forwarding does not install missing kernel drivers or grant device permissions.
- Follow [per-arm setup](hardware.md). Use the serial path visible **inside WSL**,
  preferably `/dev/serial/by-id/`, rather than a Windows COM port. Check each
  arm's identity after reconnecting; VID:PID identifies a device type, not an arm.
- For cameras, see [LiveKit Cameras](https://github.com/Hebbian-Robotics/livekit-cameras)
  for browser capture options and [RUA camera setup](cameras.md) for connecting
  streams to the agent. A USB camera captured inside WSL also needs forwarding
  and a compatible kernel driver; browser capture can avoid that requirement.

Keep the arm supported with torque off during setup. USB forwarding has not been
benchmarked for RUA control loops; verify timing and disconnect behavior on your
hardware before relying on it.
