use std::io;
use tun_rs::{AsyncDevice, DeviceBuilder, Layer};

/// Larger than any real frame a NIC would hand us — generous enough to
/// cover jumbo frames without needing to tune this against a real MTU yet.
const MAX_FRAME_LEN: usize = 65536;

/// A single switch port backed by a real Linux TAP device — Layer 2, so
/// `recv`/`send` move whole Ethernet frames, 802.1Q tag and all, with no
/// header of their own added or stripped.
pub struct TapPort {
    device: AsyncDevice,
}

impl TapPort {
    /// Opens (creating it if it doesn't already exist) a TAP interface
    /// named `name` in Layer-2 mode.
    ///
    /// # Errors
    ///
    /// Returns an error if the device can't be created — most commonly
    /// because the process lacks `CAP_NET_ADMIN`.
    pub fn open(name: &str) -> io::Result<Self> {
        let device = DeviceBuilder::new()
            .name(name)
            .layer(Layer::L2)
            .build_async()?;
        Ok(Self { device })
    }

    /// The interface's kernel-visible name.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `ioctl` fails.
    pub fn name(&self) -> io::Result<String> {
        self.device.name()
    }

    /// Reads the next raw Ethernet frame off the wire.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying read fails.
    pub async fn recv(&self) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = self.device.recv(&mut buf).await?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Writes a raw Ethernet frame to the wire.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying write fails.
    pub async fn send(&self, frame: &[u8]) -> io::Result<usize> {
        self.device.send(frame).await
    }
}
