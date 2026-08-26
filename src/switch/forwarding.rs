use super::error::SwitchError;
use super::mac_table::MacTable;
use super::port::{PortId, Vlan};
use crate::frame::EthernetFrame;
use std::collections::HashMap;

pub const BROADCAST: [u8; 6] = [0xFF; 6];

/// The I/G bit is the least-significant bit of a MAC address's first octet.
/// Set, it marks a multicast/broadcast (group) address — never valid as a
/// frame's source, so a forged one must not be learned.
fn is_group_address(mac: [u8; 6]) -> bool {
    mac[0] & 0x01 != 0
}

/// What a switch decided to do with a frame after learning from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forward {
    /// Deliver to exactly this port — the destination MAC was already learned
    /// on a different port in the same VLAN.
    Unicast(PortId),
    /// Deliver to every other port in the ingress port's VLAN: destination
    /// unknown, or broadcast. Sorted by `PortId` for deterministic output.
    Flood(Vec<PortId>),
    /// The destination was learned on the same port the frame arrived on —
    /// it's already reachable there directly, so there's nothing to do.
    Drop,
}

/// Phase 2's switch core: access ports only (one VLAN per port, no
/// tag/untag), zero I/O. Ports are just handles the caller assigns meaning
/// to — actually moving bytes per a `Forward` decision is the caller's job.
#[derive(Default)]
pub struct Switch {
    ports: HashMap<PortId, Vlan>,
    mac_table: MacTable,
}

impl Switch {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `port` in `vlan`. Calling this again for a `port` that's
    /// already registered (e.g. to move it to a different VLAN) purges any
    /// MAC-table entries learned against it, so a stale route can't leak
    /// traffic into its new VLAN.
    pub fn add_port(&mut self, port: PortId, vlan: Vlan) {
        self.mac_table.remove_port(port);
        self.ports.insert(port, vlan);
    }

    /// Deregisters `port` and purges its learned MAC-table entries. A later
    /// `forward` call using this `port` as ingress returns `UnknownPort`,
    /// and it drops out of every VLAN's flood set.
    pub fn remove_port(&mut self, port: PortId) {
        self.ports.remove(&port);
        self.mac_table.remove_port(port);
    }

    /// Learns `frame`'s source MAC against `ingress`'s VLAN, then decides
    /// where the frame goes.
    ///
    /// # Errors
    ///
    /// Returns [`SwitchError::UnknownPort`] if `ingress` was never
    /// registered via [`Switch::add_port`].
    pub fn forward(
        &mut self,
        ingress: PortId,
        frame: &EthernetFrame,
    ) -> Result<Forward, SwitchError> {
        let vlan = *self
            .ports
            .get(&ingress)
            .ok_or(SwitchError::UnknownPort(ingress))?;

        // A forged multicast/broadcast source is never learned — which, as a
        // side effect, also keeps a later multicast/broadcast *destination*
        // lookup from ever matching a learned unicast entry.
        if !is_group_address(frame.src) {
            self.mac_table.learn(vlan, frame.src, ingress);
        }

        let egress = if frame.dst == BROADCAST {
            None
        } else {
            self.mac_table.lookup(vlan, frame.dst)
        };

        Ok(match egress {
            Some(egress) if egress == ingress => Forward::Drop,
            Some(egress) => Forward::Unicast(egress),
            None => Forward::Flood(self.flood_targets(vlan, ingress)),
        })
    }

    fn flood_targets(&self, vlan: Vlan, exclude: PortId) -> Vec<PortId> {
        let mut targets: Vec<PortId> = self
            .ports
            .iter()
            .filter_map(|(&id, &v)| (v == vlan && id != exclude).then_some(id))
            .collect();
        targets.sort_by_key(|p| p.0);
        targets
    }
}
