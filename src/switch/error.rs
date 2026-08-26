use super::port::{PortId, Vlan};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SwitchError {
    /// `forward` was called with a port never registered via `add_port`.
    #[error("unknown port: {0:?}")]
    UnknownPort(PortId),
    /// A trunk port received a tagged frame for a VLAN not in its allowed
    /// set and not its native VLAN.
    #[error("{port:?}: VLAN {vlan} not allowed on this trunk")]
    VlanNotAllowedOnTrunk { port: PortId, vlan: Vlan },
    /// A trunk port received an untagged frame but has no native VLAN
    /// configured to associate it with.
    #[error("{port:?}: untagged frame on a trunk with no native VLAN")]
    UntaggedFrameOnTrunkWithoutNative { port: PortId },
    /// An access port received a tagged frame — access ports are
    /// untagged-only by definition, so this is a protocol violation, not
    /// something to silently accept.
    #[error("{port:?}: tagged frame on an access port")]
    TaggedFrameOnAccessPort { port: PortId },
    /// `forward` was called with a port the loop guard has blocked — see
    /// [`super::Switch::block_port`].
    #[error("{0:?}: port is blocked by the loop guard")]
    PortBlocked(PortId),
}
