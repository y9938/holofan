//! USB HID communication module (VID: 0x0483, PID: 0x5750).

use anyhow::Result;

/// Vendor ID (STM32 USB HID).
pub const VENDOR_ID: u16 = 0x0483;
/// Product ID for holographic fan controller.
pub const PRODUCT_ID: u16 = 0x5750;

/// Device connection status and raw status telemetry.
#[derive(Debug, Clone)]
pub struct DeviceStatus {
    pub connected: bool,
    pub raw_response: [u8; 64],
}

#[cfg(feature = "hid")]
mod imp {
    use super::*;
    use anyhow::{bail, Context};
    use hidapi::{HidApi, HidDevice};

    pub struct FanDevice {
        device: HidDevice,
    }

    impl FanDevice {
        /// Connects to the fan device over USB HID.
        pub fn connect() -> Result<Self> {
            let api = HidApi::new().context("Failed to initialize HID API")?;
            let device = api.open(VENDOR_ID, PRODUCT_ID).with_context(|| {
                format!(
                    "Fan device not detected (VID={:#06x}, PID={:#06x})",
                    VENDOR_ID, PRODUCT_ID
                )
            })?;
            Ok(Self { device })
        }

        /// Sends a 60-byte command packet prepended with Report ID = 0.
        pub fn send_command(&self, packet: &[u8; 60]) -> Result<()> {
            let mut report = [0u8; 61];
            report[0] = 0; // Report ID = 0
            report[1..].copy_from_slice(packet);
            self.device
                .write(&report)
                .with_context(|| "Failed to write command to USB HID device")?;
            Ok(())
        }

        /// Reads a 64-byte reply from the device.
        pub fn read_reply(&self) -> Result<[u8; 64]> {
            let mut buf = [0u8; 64];
            let read_bytes = self
                .device
                .read(&mut buf)
                .with_context(|| "Failed to read reply from USB HID device")?;
            if read_bytes == 0 {
                bail!("Device reply timeout");
            }
            Ok(buf)
        }

        /// Queries device status (command 0x40CA6C).
        pub fn query_status(&self) -> Result<DeviceStatus> {
            let mut cmd = [0u8; 60];
            cmd[0..4].copy_from_slice(&0x40ca6cu32.to_le_bytes());
            self.send_command(&cmd)?;
            let reply = self.read_reply()?;
            Ok(DeviceStatus {
                connected: true,
                raw_response: reply,
            })
        }

        /// Reads a single data block by index.
        pub fn read_block(&self, block_idx: u8) -> Result<Vec<u8>> {
            let mut cmd = [0u8; 60];
            cmd[9] = block_idx;
            self.send_command(&cmd)?;
            let reply = self.read_reply()?;
            Ok(reply[1..61].to_vec())
        }
    }
}

#[cfg(not(feature = "hid"))]
mod imp {
    use super::*;
    use anyhow::bail;

    pub struct FanDevice;

    impl FanDevice {
        pub fn connect() -> Result<Self> {
            bail!("Direct USB HID support disabled in builds without the 'hid' feature");
        }

        pub fn query_status(&self) -> Result<DeviceStatus> {
            bail!("Direct USB HID support disabled in builds without the 'hid' feature");
        }

        pub fn read_block(&self, _block_idx: u8) -> Result<Vec<u8>> {
            bail!("Direct USB HID support disabled in builds without the 'hid' feature");
        }
    }
}

pub use imp::FanDevice;
