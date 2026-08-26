use std::collections::HashSet;
use std::fmt;

/// VLAN identifier. 1..=4094 are assignable; 0 means "priority-tagged, no
/// VLAN" and 4095 is reserved — [`PortMode::access`] and
/// [`PortMode::trunk`] reject both, now that phase 4 actually parses real
/// VIDs off the wire and out of config instead of just round-tripping them.
pub type Vlan = u16;

/// Identifies a port on a [`Switch`](crate::switch::Switch). Zero I/O at
/// this layer, so this is just an opaque handle the caller assigns meaning
/// to (a real interface, once phase 3's TAP layer is wired up to it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortId(pub u32);

/// `PortMode::access`/`PortMode::trunk` were given a VLAN id outside the
/// assignable 1..=4094 range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidVlan(pub Vlan);

impl fmt::Display for InvalidVlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid VLAN id {}: must be in 1..=4094 (0 is priority-tagged-only, 4095 is reserved)",
            self.0
        )
    }
}

impl std::error::Error for InvalidVlan {}

const fn is_assignable(vlan: Vlan) -> bool {
    vlan != 0 && vlan != 4095
}

/// How a port participates in VLANs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortMode {
    /// Untagged wire traffic, permanently associated with one VLAN. A
    /// tagged frame arriving here is a protocol violation, not silently
    /// accepted.
    Access { vlan: Vlan },
    /// Tagged wire traffic for every VLAN in `allowed`, plus optionally one
    /// untagged VLAN (`native`) carried without a tag in either direction.
    Trunk {
        native: Option<Vlan>,
        allowed: HashSet<Vlan>,
    },
}

impl PortMode {
    /// # Errors
    ///
    /// Returns [`InvalidVlan`] if `vlan` is outside 1..=4094.
    pub fn access(vlan: Vlan) -> Result<Self, InvalidVlan> {
        if is_assignable(vlan) {
            Ok(PortMode::Access { vlan })
        } else {
            Err(InvalidVlan(vlan))
        }
    }

    /// `allowed` may be empty if `native` is set (an untagged-only trunk,
    /// unusual but not invalid) — but not both, since that port would carry
    /// nothing at all.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidVlan`] if `native` or any VLAN in `allowed` is
    /// outside 1..=4094.
    pub fn trunk(
        native: Option<Vlan>,
        allowed: impl IntoIterator<Item = Vlan>,
    ) -> Result<Self, InvalidVlan> {
        if let Some(v) = native
            && !is_assignable(v)
        {
            return Err(InvalidVlan(v));
        }
        let allowed: HashSet<Vlan> = allowed.into_iter().collect();
        if let Some(&v) = allowed.iter().find(|&&v| !is_assignable(v)) {
            return Err(InvalidVlan(v));
        }
        Ok(PortMode::Trunk { native, allowed })
    }

    /// Whether a frame belonging to `vlan` may cross this port at all — as
    /// a flood target, and as the set of VLANs a trunk will accept a
    /// tagged frame for.
    pub(crate) fn carries(&self, vlan: Vlan) -> bool {
        match self {
            PortMode::Access { vlan: v } => *v == vlan,
            PortMode::Trunk { native, allowed } => allowed.contains(&vlan) || *native == Some(vlan),
        }
    }
}
