//! Trust model: a probe id is broadcast in the clear out every port,
//! including access ports facing untrusted end hosts, every
//! `LOOP_PROBE_INTERVAL`. Anything on the wire can read it and echo it
//! straight back, blocking its own ingress port — self-harm on an access
//! port, but a one-frame, segment-wide `DoS` if replayed from behind a
//! trunk. Real switches hit the same problem on BPDU guard and answer it
//! with `errdisable` auto-recovery after a timeout; this crate has no
//! such timeout today (recovery is SIGHUP-only), which is an accepted gap
//! for a lightweight self-loop guard rather than full spanning tree — but
//! it does mean the blast radius of a replayed probe is "until an
//! operator intervenes," not bounded automatically.
use crate::frame::EthernetFrame;

/// Reserved `EtherType` for vlan-rs's own loop-detection probes — never
/// real user traffic. Recognized and pulled out of the data path entirely
/// before any VLAN/tag processing, the same way real switches special-case
/// BPDUs rather than subjecting them to normal forwarding rules.
///
/// `0x88B7` is IANA/IEEE-assigned as "IEEE Std 802 - OUI Extended
/// `EtherType`" — not a free-for-use slot, and these probes aren't
/// conformant OUI-extended frames. Sharing it is a shortcut, not a claim
/// of ownership: [`PROBE_MAGIC`] is what actually distinguishes a probe
/// from unrelated `0x88B7` traffic transiting this switch, so that
/// traffic still falls through to normal forwarding instead of being
/// silently black-holed here.
const PROBE_ETHERTYPE: u16 = 0x88B7;

/// Marks a payload as belonging to vlan-rs's own probe protocol, distinct
/// from any other traffic that happens to use [`PROBE_ETHERTYPE`]. Chosen
/// arbitrarily; only needs to be unlikely to collide with real payloads.
const PROBE_MAGIC: [u8; 4] = *b"VLPB";

/// Total probe payload size: [`PROBE_MAGIC`] (4) + the probe id (8) +
/// zero padding, sized so an untagged probe frame (14-byte header) lands
/// exactly on the 802.3 minimum frame size of 60 bytes. Real hardware
/// pads any shorter frame to that minimum on the wire; without matching
/// padding here, a probe built at its "natural" 12-byte payload arrives
/// back at this switch already padded to 46 bytes by the NIC, and a
/// naive length-exact payload match would then reject it as not a probe
/// at all — the loop guard silently doing nothing on real hardware, even
/// though every unit test (TAP-to-TAP with no real NIC in the path) stays
/// green. [`payload_probe_id`] reads only the first 12 bytes for exactly
/// this reason: it stays correct whether or not the peer padded.
const PROBE_PAYLOAD_LEN: usize = 46;

/// Locally-administered *unicast* source MAC prefix for probe frames (U/L
/// bit set, I/G bit clear). Deliberately **not** a multicast source: real
/// bridges — including Linux's, in `br_handle_frame()` — drop any frame
/// whose source address has the multicast bit set as invalid
/// (`is_valid_ether_addr`), so a multicast-sourced probe would never
/// survive a hop across a real link and could never be detected looping
/// back. Probes never reach MAC learning regardless (`forward` handles
/// them before that point — see its doc comment), so there was never a
/// need to piggyback on the "never learn a multicast source" rule here.
const PROBE_SRC_PREFIX: u8 = 0x02;

/// Generates a probe id astronomically unlikely to collide between two
/// switch instances on the same segment — a collision would mean each
/// mistakes the other's probe for its own looped-back one, a false
/// positive. Two sources of entropy, mixed together, so this doesn't rest
/// on either alone: `RandomState`'s own OS-seeded random key (its
/// documented purpose — resisting `HashDoS` — happens to also give distinct
/// keys per `RandomState::new()` call today, but that's unspecified `std`
/// behavior, not a guarantee), and a process-wide call counter, which
/// *is* guaranteed distinct per call. If a future `std` ever made
/// `RandomState::new()` reuse a key, every switch in the same process
/// would still get a different id from the counter alone. This needs no
/// `rand` dependency.
pub(crate) fn random_probe_id() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;
    use std::sync::atomic::{AtomicU64, Ordering};

    static CALLS: AtomicU64 = AtomicU64::new(0);
    let call = CALLS.fetch_add(1, Ordering::Relaxed);

    RandomState::new().hash_one(call)
}

/// Builds the wire bytes for a loop-detection probe carrying `probe_id`,
/// padded out to [`PROBE_PAYLOAD_LEN`] so the frame reaches the 802.3
/// minimum on the wire — see that constant's docs for why an unpadded
/// probe would defeat its own detection on real hardware.
pub(crate) fn build_probe(probe_id: u64) -> Vec<u8> {
    let mut src = [0u8; 6];
    src[0] = PROBE_SRC_PREFIX;
    src[1..6].copy_from_slice(&probe_id.to_be_bytes()[0..5]);
    let mut payload = [0u8; PROBE_PAYLOAD_LEN];
    payload[0..4].copy_from_slice(&PROBE_MAGIC);
    payload[4..12].copy_from_slice(&probe_id.to_be_bytes());
    let frame = EthernetFrame {
        dst: [0xFF; 6],
        src,
        tag: None,
        ethertype: PROBE_ETHERTYPE,
        payload: &payload,
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
/// Checks [`PROBE_MAGIC`], not just the `EtherType` — `0x88B7` isn't ours
/// to claim outright (see [`PROBE_ETHERTYPE`]'s docs), so unrelated
/// traffic that happens to share it must still fall through to normal
/// forwarding rather than being swallowed here.
pub(crate) fn is_any_probe(frame: &EthernetFrame) -> bool {
    frame.ethertype == PROBE_ETHERTYPE && frame.payload.get(0..4) == Some(&PROBE_MAGIC[..])
}

/// Reads the probe id out of a probe payload — tolerant of trailing bytes
/// (real-hardware padding) as long as the leading [`PROBE_MAGIC`] and the
/// 8 id bytes right after it are present. `None` if `payload` is too
/// short or doesn't start with the magic, i.e. isn't actually a probe.
fn payload_probe_id(payload: &[u8]) -> Option<u64> {
    if payload.get(0..4) != Some(&PROBE_MAGIC[..]) {
        return None;
    }
    let id_bytes: [u8; 8] = payload.get(4..12)?.try_into().ok()?;
    Some(u64::from_be_bytes(id_bytes))
}
