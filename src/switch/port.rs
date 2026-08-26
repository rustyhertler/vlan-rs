/// VLAN identifier. On the wire, 1..=4094 are assignable (0 means
/// "priority-tagged, no VLAN"; 4095 is reserved) — `Switch::add_port` doesn't
/// enforce that range yet, since phase 2 has no tagged input to check it
/// against. Revisit once trunk ports (phase 4) start parsing real VIDs.
pub type Vlan = u16;

/// Identifies a port on a [`Switch`](crate::switch::Switch). Phase 2 has no
/// I/O, so this is just an opaque handle the caller assigns meaning to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortId(pub u32);
