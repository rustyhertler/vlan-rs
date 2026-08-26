use std::collections::HashSet;
use thiserror::Error;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid VLAN id {0}: must be in 1..=4094 (0 is priority-tagged-only, 4095 is reserved)")]
pub struct InvalidVlan(pub Vlan);

/// [`PortMode::trunk`] rejected its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PortModeError {
    /// A VLAN id was outside the assignable 1..=4094 range.
    #[error(
        "invalid VLAN id {0}: must be in 1..=4094 (0 is priority-tagged-only, 4095 is reserved)"
    )]
    InvalidVlan(Vlan),
    /// Neither a native VLAN nor any allowed VLAN was given — this trunk
    /// could never carry anything.
    #[error("a trunk needs a native VLAN, at least one allowed VLAN, or both")]
    EmptyTrunk,
}

impl From<InvalidVlan> for PortModeError {
    fn from(e: InvalidVlan) -> Self {
        PortModeError::InvalidVlan(e.0)
    }
}

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
    /// nothing at all; enforced here, not just by callers, so no caller
    /// (the CLI today, phase 5's TOML config loader tomorrow) can construct
    /// a structurally-useless trunk by skipping its own check.
    ///
    /// # Errors
    ///
    /// Returns [`PortModeError::InvalidVlan`] if `native` or any VLAN in
    /// `allowed` is outside 1..=4094, or [`PortModeError::EmptyTrunk`] if
    /// both `native` and `allowed` are empty.
    pub fn trunk(
        native: Option<Vlan>,
        allowed: impl IntoIterator<Item = Vlan>,
    ) -> Result<Self, PortModeError> {
        if let Some(v) = native
            && !is_assignable(v)
        {
            return Err(PortModeError::InvalidVlan(v));
        }
        let allowed: HashSet<Vlan> = allowed.into_iter().collect();
        if let Some(&v) = allowed.iter().find(|&&v| !is_assignable(v)) {
            return Err(PortModeError::InvalidVlan(v));
        }
        if native.is_none() && allowed.is_empty() {
            return Err(PortModeError::EmptyTrunk);
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
