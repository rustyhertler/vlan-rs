use super::port::{PortId, Vlan};
use std::collections::HashMap;

/// Learned MAC -> port mappings, scoped per VLAN so learning in one VLAN can
/// never resolve a lookup in another — that scoping *is* the isolation
/// guarantee, not an add-on check elsewhere.
#[derive(Default)]
pub(crate) struct MacTable {
    entries: HashMap<(Vlan, [u8; 6]), PortId>,
}

impl MacTable {
    pub(crate) fn learn(&mut self, vlan: Vlan, mac: [u8; 6], port: PortId) {
        self.entries.insert((vlan, mac), port);
    }

    pub(crate) fn lookup(&self, vlan: Vlan, mac: [u8; 6]) -> Option<PortId> {
        self.entries.get(&(vlan, mac)).copied()
    }
}
