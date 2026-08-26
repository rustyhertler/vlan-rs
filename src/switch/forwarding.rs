use super::error::SwitchError;
use super::mac_table::MacTable;
use super::port::{PortId, Vlan};
use crate::frame::EthernetFrame;
use std::collections::HashMap;

const BROADCAST: [u8; 6] = [0xFF; 6];

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

    pub fn add_port(&mut self, port: PortId, vlan: Vlan) {
        self.ports.insert(port, vlan);
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

        self.mac_table.learn(vlan, frame.src, ingress);

        if frame.dst == BROADCAST {
            return Ok(Forward::Flood(self.flood_targets(vlan, ingress)));
        }

        Ok(match self.mac_table.lookup(vlan, frame.dst) {
            Some(egress) if egress == ingress => Forward::Drop,
            Some(egress) => Forward::Unicast(egress),
            None => Forward::Flood(self.flood_targets(vlan, ingress)),
        })
    }

    fn flood_targets(&self, vlan: Vlan, exclude: PortId) -> Vec<PortId> {
        let mut targets: Vec<PortId> = self
            .ports
            .iter()
            .filter(|&(&id, &v)| v == vlan && id != exclude)
            .map(|(&id, _)| id)
            .collect();
        targets.sort_by_key(|p| p.0);
        targets
    }
}
