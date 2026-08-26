use crate::frame::EthernetFrame;

/// Reserved `EtherType` for vlan-rs's own loop-detection probes — never
/// real user traffic. Recognized and pulled out of the data path entirely
/// before any VLAN/tag processing, the same way real switches special-case
/// BPDUs rather than subjecting them to normal forwarding rules.
const PROBE_ETHERTYPE: u16 = 0x88B7;

/// Locally-administered + multicast source MAC for probe frames, so the
/// existing "never learn a multicast source" rule (see `forwarding.rs`)
/// keeps them out of the MAC table with no special-casing needed here.
const PROBE_SRC_PREFIX: u8 = 0x03;

/// Generates a probe id astronomically unlikely to collide between two
/// switch instances on the same segment — a collision would mean each
/// mistakes the other's probe for its own looped-back one, a false
/// positive. `SipHash` keyed by `RandomState`'s own OS-seeded random key,
/// finalized over zero bytes: the key alone (not the empty input) is what
/// makes the output vary, so this needs no `rand` dependency. Verified
/// empirically to vary both within and across processes before relying on
/// it here.
pub(crate) fn random_probe_id() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}

/// Builds the wire bytes for a loop-detection probe carrying `probe_id`.
pub(crate) fn build_probe(probe_id: u64) -> Vec<u8> {
    let mut src = [0u8; 6];
    src[0] = PROBE_SRC_PREFIX;
    src[1..6].copy_from_slice(&probe_id.to_be_bytes()[0..5]);
    let frame = EthernetFrame {
        dst: [0xFF; 6],
        src,
        tag: None,
        ethertype: PROBE_ETHERTYPE,
        payload: &probe_id.to_be_bytes(),
    };
    let mut bytes = Vec::new();
    // write_into only ever fails for an untagged frame whose EtherType is
    // 0x8100; PROBE_ETHERTYPE isn't that, so this can't actually happen —
    // but the ignored Result stays honest about write_into's real
    // signature rather than unwrapping something merely believed safe.
    let _ = frame.write_into(&mut bytes);
    bytes
}

/// `frame` is one of *this switch's own* probes if its `EtherType` matches
/// and its payload's probe id matches `probe_id` — meaning it looped back
/// to this same switch instance, not just some other switch's probe
/// arriving on a shared segment (a normal, non-loop situation).
pub(crate) fn is_own_probe(frame: &EthernetFrame, probe_id: u64) -> bool {
    frame.ethertype == PROBE_ETHERTYPE && payload_probe_id(frame.payload) == Some(probe_id)
}

/// Whether `frame` is a loop-guard probe at all, regardless of whose.
pub(crate) fn is_any_probe(frame: &EthernetFrame) -> bool {
    frame.ethertype == PROBE_ETHERTYPE
}

fn payload_probe_id(payload: &[u8]) -> Option<u64> {
    <[u8; 8]>::try_from(payload).ok().map(u64::from_be_bytes)
}
