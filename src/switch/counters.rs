/// Frame/byte counters for one port or one VLAN. Same shape for both — the
/// switch core tracks a set of these per `PortId` and per `Vlan`, both
/// updated from the same `forward` call so they stay consistent with each
/// other.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    pub frames_in: u64,
    pub bytes_in: u64,
    pub frames_out: u64,
    pub bytes_out: u64,
    /// Frames rejected by [`super::SwitchError`] at ingress — a VLAN/tagging
    /// policy violation, not an ordinary "no known destination" flood.
    pub drops: u64,
}
