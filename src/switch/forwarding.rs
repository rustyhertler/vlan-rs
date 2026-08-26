use super::error::SwitchError;
use super::mac_table::MacTable;
use super::port::{PortId, PortMode, Vlan};
use crate::frame::{Dot1qTag, EthernetFrame};
use std::collections::HashMap;

pub const BROADCAST: [u8; 6] = [0xFF; 6];

/// The I/G bit is the least-significant bit of a MAC address's first octet.
/// Set, it marks a multicast/broadcast (group) address — never valid as a
/// frame's source, so a forged one must not be learned.
fn is_group_address(mac: [u8; 6]) -> bool {
    mac[0] & 0x01 != 0
}

/// A frame ready to hand to `port`'s writer, already encoded exactly as
/// that port's mode requires — tagged for a trunk carrying a non-native
/// VLAN, untagged for an access port or a trunk's native VLAN.
#[derive(Debug, PartialEq, Eq)]
pub struct Delivery {
    pub port: PortId,
    pub bytes: Vec<u8>,
}

/// The switch core: per-VLAN MAC learning, access *and* trunk ports, zero
/// I/O. Ports are just handles the caller assigns meaning to — actually
/// moving bytes per the `Delivery` list `forward` returns is the caller's
/// job.
#[derive(Default)]
pub struct Switch {
    ports: HashMap<PortId, PortMode>,
    mac_table: MacTable,
}

impl Switch {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `port` in `mode`. Calling this again for a `port` that's
    /// already registered (e.g. to change its mode) purges any MAC-table
    /// entries learned against it, so a stale route can't leak traffic
    /// into whatever VLANs its new mode carries.
    pub fn add_port(&mut self, port: PortId, mode: PortMode) {
        self.mac_table.remove_port(port);
        self.ports.insert(port, mode);
    }

    /// Deregisters `port` and purges its learned MAC-table entries. A later
    /// `forward` call using this `port` as ingress returns `UnknownPort`,
    /// and it drops out of every VLAN's flood set.
    pub fn remove_port(&mut self, port: PortId) {
        self.ports.remove(&port);
        self.mac_table.remove_port(port);
    }

    /// Learns `frame`'s source MAC against `ingress`'s resolved VLAN, then
    /// decides where the frame goes — returning it pre-encoded for each
    /// egress port's mode. Empty means drop.
    ///
    /// # Errors
    ///
    /// Returns [`SwitchError::UnknownPort`] if `ingress` was never
    /// registered via [`Switch::add_port`], [`SwitchError::TaggedFrameOnAccessPort`]
    /// if a tagged frame arrived on an access port, or a trunk-specific
    /// error if `frame` doesn't resolve to a VLAN that trunk carries — see
    /// [`SwitchError`].
    pub fn forward(
        &mut self,
        ingress: PortId,
        frame: &EthernetFrame,
    ) -> Result<Vec<Delivery>, SwitchError> {
        let vlan = self.ingress_vlan(ingress, frame)?;

        // A forged multicast/broadcast source is never learned — which, as a
        // side effect, also keeps a later multicast/broadcast *destination*
        // lookup from ever matching a learned unicast entry.
        if !is_group_address(frame.src) {
            self.mac_table.learn(vlan, frame.src, ingress);
        }

        let egress_ports = if frame.dst == BROADCAST {
            self.flood_targets(vlan, ingress)
        } else {
            match self.mac_table.lookup(vlan, frame.dst) {
                Some(egress) if egress == ingress => Vec::new(),
                Some(egress) => vec![egress],
                None => self.flood_targets(vlan, ingress),
            }
        };

        Ok(egress_ports
            .into_iter()
            .filter_map(|port| self.encode_for_egress(port, vlan, frame))
            .collect())
    }

    /// Resolves the VLAN a frame arriving on `port` belongs to, per that
    /// port's mode. An access port ignores the wire entirely (it's always
    /// that one VLAN) but rejects a tagged frame outright rather than
    /// silently accepting one — a tag there means the two ends disagree
    /// about what kind of link this is.
    fn ingress_vlan(&self, port: PortId, frame: &EthernetFrame) -> Result<Vlan, SwitchError> {
        let mode = self
            .ports
            .get(&port)
            .ok_or(SwitchError::UnknownPort(port))?;
        match (mode, &frame.tag) {
            (PortMode::Access { .. }, Some(_)) => {
                Err(SwitchError::TaggedFrameOnAccessPort { port })
            }
            (PortMode::Trunk { native, allowed }, Some(tag))
                if allowed.contains(&tag.vid) || *native == Some(tag.vid) =>
            {
                Ok(tag.vid)
            }
            (PortMode::Trunk { .. }, Some(tag)) => Err(SwitchError::VlanNotAllowedOnTrunk {
                port,
                vlan: tag.vid,
            }),
            (
                PortMode::Access { vlan }
                | PortMode::Trunk {
                    native: Some(vlan), ..
                },
                None,
            ) => Ok(*vlan),
            (PortMode::Trunk { native: None, .. }, None) => {
                Err(SwitchError::UntaggedFrameOnTrunkWithoutNative { port })
            }
        }
    }

    /// Builds the frame `port` should actually receive on the wire for
    /// `vlan`: untagged for an access port or a trunk's native VLAN,
    /// 802.1Q-tagged otherwise. `None` means the frame couldn't be encoded
    /// (an unregistered port, or the rare case where an untagged frame's
    /// `EtherType` collides with the 802.1Q TPID — see
    /// [`crate::frame::WriteError`]) — dropped rather than failing the
    /// whole `forward` call over one bad target.
    fn encode_for_egress(
        &self,
        port: PortId,
        vlan: Vlan,
        frame: &EthernetFrame,
    ) -> Option<Delivery> {
        let mode = self.ports.get(&port)?;
        let tag = match mode {
            PortMode::Access { .. } => None,
            PortMode::Trunk { native, .. } if *native == Some(vlan) => None,
            PortMode::Trunk { .. } => Some(Dot1qTag {
                // Best-effort round-trip of the original priority bits when
                // the frame already carried a tag; a sane default otherwise.
                pcp: frame.tag.map_or(0, |t| t.pcp),
                dei: frame.tag.is_some_and(|t| t.dei),
                vid: vlan,
            }),
        };
        let out = EthernetFrame {
            dst: frame.dst,
            src: frame.src,
            tag,
            ethertype: frame.ethertype,
            payload: frame.payload,
        };
        let mut bytes = Vec::new();
        out.write_into(&mut bytes).ok()?;
        Some(Delivery { port, bytes })
    }

    fn flood_targets(&self, vlan: Vlan, exclude: PortId) -> Vec<PortId> {
        let mut targets: Vec<PortId> = self
            .ports
            .iter()
            .filter_map(|(&id, mode)| (id != exclude && mode.carries(vlan)).then_some(id))
            .collect();
        targets.sort_by_key(|p| p.0);
        targets
    }
}
