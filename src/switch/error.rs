use super::port::{PortId, Vlan};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchError {
    /// `forward` was called with a port never registered via `add_port`.
    UnknownPort(PortId),
    /// A trunk port received a tagged frame for a VLAN not in its allowed
    /// set and not its native VLAN.
    VlanNotAllowedOnTrunk { port: PortId, vlan: Vlan },
    /// A trunk port received an untagged frame but has no native VLAN
    /// configured to associate it with.
    UntaggedFrameOnTrunkWithoutNative { port: PortId },
    /// An access port received a tagged frame — access ports are
    /// untagged-only by definition, so this is a protocol violation, not
    /// something to silently accept.
    TaggedFrameOnAccessPort { port: PortId },
}

impl fmt::Display for SwitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SwitchError::UnknownPort(port) => write!(f, "unknown port: {port:?}"),
            SwitchError::VlanNotAllowedOnTrunk { port, vlan } => {
                write!(f, "{port:?}: VLAN {vlan} not allowed on this trunk")
            }
            SwitchError::UntaggedFrameOnTrunkWithoutNative { port } => {
                write!(f, "{port:?}: untagged frame on a trunk with no native VLAN")
            }
            SwitchError::TaggedFrameOnAccessPort { port } => {
                write!(f, "{port:?}: tagged frame on an access port")
            }
        }
    }
}

impl std::error::Error for SwitchError {}
