use std::time::Instant;

use vlan_rs::frame::{Dot1qTag, EthernetFrame};
use vlan_rs::switch::{BROADCAST, Counters, Delivery, PortId, PortMode, Switch, SwitchError};

/// Most tests don't care about actual timing — `Instant::now()` is a valid
/// `now` for any `forward()` call that isn't specifically testing aging.
fn now() -> Instant {
    Instant::now()
}

const HOST_A: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x01]; // port 1, vlan 10
const HOST_B: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x02]; // port 2, vlan 10
const HOST_C: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x03]; // port 3, vlan 20
const MULTICAST_SRC: [u8; 6] = [0x01, 0x00, 0x5E, 0x00, 0x00, 0x01]; // I/G bit set — invalid as a source

const PORT1: PortId = PortId(1);
const PORT2: PortId = PortId(2);
const PORT3: PortId = PortId(3);

fn frame(dst: [u8; 6], src: [u8; 6]) -> EthernetFrame<'static> {
    EthernetFrame {
        dst,
        src,
        tag: None,
        ethertype: 0x0800,
        payload: &[],
    }
}

fn tagged_frame(dst: [u8; 6], src: [u8; 6], vid: u16) -> EthernetFrame<'static> {
    EthernetFrame {
        dst,
        src,
        tag: Some(Dot1qTag {
            pcp: 0,
            dei: false,
            vid,
        }),
        ethertype: 0x0800,
        payload: &[],
    }
}

fn ports_of(deliveries: &[Delivery]) -> Vec<PortId> {
    let mut ports: Vec<PortId> = deliveries.iter().map(|d| d.port).collect();
    ports.sort_by_key(|p| p.0);
    ports
}

/// Ports 1 & 2 in VLAN 10, port 3 alone in VLAN 20 — all access.
fn two_vlans() -> Switch {
    let mut switch = Switch::new();
    switch.add_port(PORT1, PortMode::access(10).unwrap());
    switch.add_port(PORT2, PortMode::access(10).unwrap());
    switch.add_port(PORT3, PortMode::access(20).unwrap());
    switch
}

#[test]
fn floods_broadcast_only_within_ingress_vlan() {
    let mut switch = two_vlans();
    let deliveries = switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), now())
        .unwrap();
    assert_eq!(ports_of(&deliveries), vec![PORT2]);
}

#[test]
fn floods_unknown_unicast_only_within_ingress_vlan() {
    let mut switch = two_vlans();
    // nobody has been learned yet, so an unknown dst still just floods vlan 10
    let deliveries = switch
        .forward(PORT1, &frame(HOST_B, HOST_A), now())
        .unwrap();
    assert_eq!(ports_of(&deliveries), vec![PORT2]);
}

#[test]
fn delivers_unicast_once_destination_is_learned() {
    let mut switch = two_vlans();
    // host B speaks first, so the switch learns it's behind port 2
    switch
        .forward(PORT2, &frame(BROADCAST, HOST_B), now())
        .unwrap();

    let deliveries = switch
        .forward(PORT1, &frame(HOST_B, HOST_A), now())
        .unwrap();
    assert_eq!(ports_of(&deliveries), vec![PORT2]);
}

#[test]
fn same_mac_in_different_vlans_never_crosses_vlans() {
    let mut switch = two_vlans();
    // host C, on the vlan-20 port, is learned there
    switch
        .forward(PORT3, &frame(BROADCAST, HOST_C), now())
        .unwrap();

    // vlan 10 has never seen host C's MAC — the (vlan, mac) key keeps it
    // that way — so a vlan-10 port targeting it must still flood *within
    // vlan 10*, and can never resolve to port 3.
    let deliveries = switch
        .forward(PORT1, &frame(HOST_C, HOST_A), now())
        .unwrap();
    assert_eq!(ports_of(&deliveries), vec![PORT2]);
}

#[test]
fn drops_when_destination_learned_on_ingress_port() {
    let mut switch = two_vlans();
    switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), now())
        .unwrap();

    let deliveries = switch
        .forward(PORT1, &frame(HOST_A, HOST_A), now())
        .unwrap();
    assert!(deliveries.is_empty());
}

#[test]
fn rejects_unknown_ingress_port() {
    let mut switch = two_vlans();
    let err = switch
        .forward(PortId(99), &frame(BROADCAST, HOST_A), now())
        .unwrap_err();
    assert_eq!(err, SwitchError::UnknownPort(PortId(99)));
}

#[test]
fn reassigning_a_port_purges_its_stale_routes() {
    let mut switch = two_vlans();
    // host A is learned behind port 1, vlan 10
    switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), now())
        .unwrap();
    assert_eq!(
        ports_of(
            &switch
                .forward(PORT2, &frame(HOST_A, HOST_B), now())
                .unwrap()
        ),
        vec![PORT1]
    );

    // port 1 gets re-provisioned into vlan 20 — its old vlan-10 route to
    // host A must not survive, or vlan-10 traffic could still reach it
    switch.add_port(PORT1, PortMode::access(20).unwrap());

    let deliveries = switch
        .forward(PORT2, &frame(HOST_A, HOST_B), now())
        .unwrap();
    assert!(
        deliveries.is_empty(),
        "port 1 left vlan 10, so vlan 10 has no other member to flood to \
         — and it must never resolve to port 1 again"
    );
}

#[test]
fn removed_port_is_unknown_and_drops_out_of_flooding() {
    let mut switch = two_vlans();
    switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), now())
        .unwrap();

    switch.remove_port(PORT1);

    let err = switch
        .forward(PORT1, &frame(BROADCAST, HOST_B), now())
        .unwrap_err();
    assert_eq!(err, SwitchError::UnknownPort(PORT1));

    // vlan 10's only remaining member is port 2, so a flood from it has
    // nowhere left to go
    let deliveries = switch
        .forward(PORT2, &frame(BROADCAST, HOST_B), now())
        .unwrap();
    assert!(deliveries.is_empty());
}

#[test]
fn never_learns_a_multicast_or_broadcast_source() {
    let mut switch = two_vlans();
    // a frame claiming to be *from* a multicast address is malformed —
    // the switch must not treat it as a real host behind port 1
    switch
        .forward(PORT1, &frame(HOST_A, MULTICAST_SRC), now())
        .unwrap();

    // so a later frame *to* that same multicast address still floods,
    // rather than resolving to a bogus single unicast port
    let deliveries = switch
        .forward(PORT2, &frame(MULTICAST_SRC, HOST_B), now())
        .unwrap();
    assert_eq!(ports_of(&deliveries), vec![PORT1]);
}

/// The roadmap's phase-2 acceptance criterion: prove VLAN isolation with
/// real in-process channels standing in for ports, no kernel involved.
#[test]
fn vlan_isolation_over_channels() {
    use std::sync::mpsc;

    let (tx1, rx1) = mpsc::channel::<Vec<u8>>();
    let (tx2, rx2) = mpsc::channel::<Vec<u8>>();
    let (tx3, rx3) = mpsc::channel::<Vec<u8>>();
    let senders = [(PORT1, tx1), (PORT2, tx2), (PORT3, tx3)];

    let mut switch = two_vlans();

    let mut wire = Vec::new();
    frame(BROADCAST, HOST_A).write_into(&mut wire).unwrap();

    let parsed = EthernetFrame::parse(&wire).unwrap();
    let deliveries = switch.forward(PORT1, &parsed, now()).unwrap();
    for Delivery { port, bytes } in deliveries {
        if let Some((_, tx)) = senders.iter().find(|(id, _)| *id == port) {
            tx.send(bytes).unwrap();
        }
    }

    assert_eq!(rx2.try_recv().unwrap(), wire);
    assert!(
        rx1.try_recv().is_err(),
        "never echoed back to the ingress port"
    );
    assert!(
        rx3.try_recv().is_err(),
        "vlan 20 must never see vlan 10's broadcast"
    );
}

// --- Trunk ports (phase 4) ---

const TRUNK: PortId = PortId(10);
const ACCESS10: PortId = PortId(11);
const ACCESS20: PortId = PortId(12);

#[test]
fn port_mode_trunk_rejects_neither_native_nor_allowed() {
    // enforced by the constructor itself, not just the CLI parser layered
    // on top of it — a structurally-useless trunk (carries nothing) should
    // be unconstructable by any caller, present or future
    assert!(PortMode::trunk(None, []).is_err());
}

#[test]
fn trunk_egress_leaves_native_vlan_untagged() {
    let mut switch = Switch::new();
    switch.add_port(ACCESS10, PortMode::access(10).unwrap());
    switch.add_port(TRUNK, PortMode::trunk(Some(10), [10, 20]).unwrap());

    let deliveries = switch
        .forward(ACCESS10, &frame(BROADCAST, HOST_A), now())
        .unwrap();
    let delivery = deliveries.iter().find(|d| d.port == TRUNK).unwrap();
    let out = EthernetFrame::parse(&delivery.bytes).unwrap();
    assert!(
        out.tag.is_none(),
        "native VLAN must cross the trunk untagged"
    );
}

#[test]
fn trunk_egress_tags_non_native_vlan() {
    let mut switch = Switch::new();
    switch.add_port(ACCESS20, PortMode::access(20).unwrap());
    switch.add_port(TRUNK, PortMode::trunk(Some(10), [10, 20]).unwrap());

    let deliveries = switch
        .forward(ACCESS20, &frame(BROADCAST, HOST_C), now())
        .unwrap();
    let delivery = deliveries.iter().find(|d| d.port == TRUNK).unwrap();
    let out = EthernetFrame::parse(&delivery.bytes).unwrap();
    let tag = out
        .tag
        .expect("non-native VLAN must be tagged on the trunk");
    assert_eq!(tag.vid, 20);
}

#[test]
fn trunk_ingress_resolves_tagged_frame_to_its_vid() {
    let mut switch = Switch::new();
    switch.add_port(TRUNK, PortMode::trunk(Some(10), [10, 20]).unwrap());
    switch.add_port(ACCESS20, PortMode::access(20).unwrap());

    let deliveries = switch
        .forward(TRUNK, &tagged_frame(BROADCAST, HOST_C, 20), now())
        .unwrap();
    assert_eq!(ports_of(&deliveries), vec![ACCESS20]);
    let out = EthernetFrame::parse(&deliveries[0].bytes).unwrap();
    assert!(out.tag.is_none(), "delivered untagged to an access port");
}

#[test]
fn trunk_ingress_resolves_untagged_frame_to_native_vlan() {
    let mut switch = Switch::new();
    switch.add_port(TRUNK, PortMode::trunk(Some(10), [10, 20]).unwrap());
    switch.add_port(ACCESS10, PortMode::access(10).unwrap());

    let deliveries = switch
        .forward(TRUNK, &frame(BROADCAST, HOST_A), now())
        .unwrap();
    assert_eq!(ports_of(&deliveries), vec![ACCESS10]);
}

#[test]
fn trunk_ingress_rejects_a_vlan_not_in_its_allowed_set() {
    let mut switch = Switch::new();
    switch.add_port(TRUNK, PortMode::trunk(Some(10), [10, 20]).unwrap());

    let err = switch
        .forward(TRUNK, &tagged_frame(BROADCAST, HOST_A, 30), now())
        .unwrap_err();
    assert_eq!(
        err,
        SwitchError::VlanNotAllowedOnTrunk {
            port: TRUNK,
            vlan: 30
        }
    );
}

#[test]
fn trunk_ingress_rejects_untagged_without_a_native_vlan() {
    let mut switch = Switch::new();
    switch.add_port(TRUNK, PortMode::trunk(None, [10, 20]).unwrap());

    let err = switch
        .forward(TRUNK, &frame(BROADCAST, HOST_A), now())
        .unwrap_err();
    assert_eq!(
        err,
        SwitchError::UntaggedFrameOnTrunkWithoutNative { port: TRUNK }
    );
}

#[test]
fn access_port_rejects_a_tagged_frame() {
    let mut switch = Switch::new();
    switch.add_port(ACCESS10, PortMode::access(10).unwrap());

    let err = switch
        .forward(ACCESS10, &tagged_frame(BROADCAST, HOST_A, 10), now())
        .unwrap_err();
    assert_eq!(err, SwitchError::TaggedFrameOnAccessPort { port: ACCESS10 });
}

#[test]
fn trunk_still_isolates_vlans() {
    let mut switch = Switch::new();
    switch.add_port(TRUNK, PortMode::trunk(Some(10), [10, 20]).unwrap());
    switch.add_port(ACCESS10, PortMode::access(10).unwrap());
    switch.add_port(ACCESS20, PortMode::access(20).unwrap());

    let deliveries = switch
        .forward(TRUNK, &tagged_frame(BROADCAST, HOST_C, 20), now())
        .unwrap();
    assert_eq!(
        ports_of(&deliveries),
        vec![ACCESS20],
        "vlan 20 traffic on the trunk must never reach the vlan-10 access port"
    );
}

// --- Counters (phase 5) ---

#[test]
fn counts_frames_and_bytes_on_a_unicast_delivery() {
    let mut switch = two_vlans();
    // host B learned behind port 2 first, so the next frame resolves to a
    // single-target unicast rather than a flood — this broadcast is itself
    // delivered to port 1 (vlan 10's only other member), so port 1 starts
    // the real test frame already carrying one frame_out/bytes_out
    let seed = frame(BROADCAST, HOST_B);
    let seed_len = seed.wire_len() as u64;
    switch.forward(PORT2, &seed, now()).unwrap();

    let f = frame(HOST_B, HOST_A);
    let wire_len = f.wire_len() as u64;
    switch.forward(PORT1, &f, now()).unwrap();

    let ingress = switch.port_counters(PORT1);
    assert_eq!(
        ingress,
        Counters {
            frames_in: 1,
            bytes_in: wire_len,
            frames_out: 1, // from the seed broadcast, above
            bytes_out: seed_len,
            drops: 0,
        }
    );

    let egress = switch.port_counters(PORT2);
    assert_eq!(egress.frames_out, 1); // the real unicast, not the seed
    assert_eq!(egress.bytes_out, wire_len);

    let vlan10 = switch.vlan_counters(10);
    assert_eq!(vlan10.frames_in, 2); // host B's broadcast, then host A's unicast
    assert_eq!(vlan10.frames_out, 2); // one delivery per forward() call
}

#[test]
fn counts_every_target_of_a_flood() {
    let mut switch = two_vlans();
    switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), now())
        .unwrap();

    // vlan 10 has ports 1 & 2; the flood delivers to port 2 only (not back
    // to the ingress port), so frames_out should be exactly 1, not 2
    assert_eq!(switch.port_counters(PORT2).frames_out, 1);
    assert_eq!(switch.port_counters(PORT1).frames_out, 0);
}

#[test]
fn counts_a_drop_without_counting_frames_in() {
    let mut switch = Switch::new();
    switch.add_port(ACCESS10, PortMode::access(10).unwrap());

    let err = switch
        .forward(ACCESS10, &tagged_frame(BROADCAST, HOST_A, 10), now())
        .unwrap_err();
    assert!(matches!(err, SwitchError::TaggedFrameOnAccessPort { .. }));

    let counters = switch.port_counters(ACCESS10);
    assert_eq!(
        counters,
        Counters {
            drops: 1,
            ..Counters::default()
        }
    );
}

#[test]
fn removing_a_port_clears_its_counters() {
    let mut switch = two_vlans();
    switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), now())
        .unwrap();
    assert_ne!(switch.port_counters(PORT1), Counters::default());

    switch.remove_port(PORT1);
    assert_eq!(switch.port_counters(PORT1), Counters::default());
}

// --- MAC aging (stretch goal) ---

#[test]
fn ages_out_stale_entries_but_keeps_fresh_ones() {
    use std::time::Duration;

    let mut switch = two_vlans();
    let t0 = now();
    // host A is learned behind port 1 at t0
    switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), t0)
        .unwrap();

    let t1 = t0 + Duration::from_secs(200);
    // host B is learned behind port 2 at t1 — still fresh at eviction time
    switch
        .forward(PORT2, &frame(BROADCAST, HOST_B), t1)
        .unwrap();

    // at t2, host A's entry (learned at t0, 301s ago) is stale; host B's
    // (learned at t1, 101s ago) is not
    let t2 = t0 + Duration::from_secs(301);
    let evicted = switch.age_out(Duration::from_secs(300), t2);
    assert_eq!(evicted, 1);

    // host A's route is gone, so targeting it now floods instead of
    // resolving to the port it used to be learned behind
    let deliveries = switch.forward(PORT2, &frame(HOST_A, HOST_B), t2).unwrap();
    assert_eq!(ports_of(&deliveries), vec![PORT1]);

    // host B's route is still fresh and should still resolve directly
    let deliveries = switch.forward(PORT1, &frame(HOST_B, HOST_A), t2).unwrap();
    assert_eq!(ports_of(&deliveries), vec![PORT2]);
}

#[test]
fn age_out_on_an_empty_table_evicts_nothing() {
    use std::time::Duration;

    let mut switch = two_vlans();
    assert_eq!(switch.age_out(Duration::from_secs(300), now()), 0);
}

#[test]
fn lookups_never_refresh_an_entrys_age() {
    use std::time::Duration;

    let mut switch = two_vlans();
    let t0 = now();
    // host A is learned once, at t0, and never speaks again
    switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), t0)
        .unwrap();

    // host B repeatedly targets host A — none of these are host A
    // *speaking*, so they must not push its last-seen time forward
    let t1 = t0 + Duration::from_secs(100);
    switch.forward(PORT2, &frame(HOST_A, HOST_B), t1).unwrap();
    let t2 = t0 + Duration::from_secs(200);
    switch.forward(PORT2, &frame(HOST_A, HOST_B), t2).unwrap();

    // host A's entry is still aged from t0, not t2, so it's stale by 301s
    // after t0 even though a lookup happened as recently as t2
    let t3 = t0 + Duration::from_secs(301);
    let evicted = switch.age_out(Duration::from_secs(300), t3);
    assert_eq!(evicted, 1);
}

// --- Loop guard (stretch goal) ---

/// Ports 1 & 2 in VLAN 10, same layout as `two_vlans`, but with a known
/// probe id instead of a random one so a test can build a probe frame and
/// know in advance whether `forward` will recognize it as this switch's own.
fn two_vlans_with_probe(probe_id: u64) -> Switch {
    let mut switch = Switch::with_probe_id(probe_id);
    switch.add_port(PORT1, PortMode::access(10).unwrap());
    switch.add_port(PORT2, PortMode::access(10).unwrap());
    switch
}

#[test]
fn a_switchs_own_probe_looping_back_blocks_the_receiving_port() {
    let mut switch = two_vlans_with_probe(0xABCD_1234);
    let probe = switch.build_loop_probe();
    let parsed = EthernetFrame::parse(&probe).unwrap();

    assert!(!switch.is_blocked(PORT1));
    let deliveries = switch.forward(PORT1, &parsed, now()).unwrap();
    assert!(
        deliveries.is_empty(),
        "a probe is never forwarded/flooded like regular traffic"
    );
    assert!(switch.is_blocked(PORT1));
}

#[test]
fn a_different_switchs_probe_does_not_block_anything() {
    let mut switch = two_vlans_with_probe(0xABCD_1234);
    // built by a *different* switch instance's probe id — this is just
    // another switch on the same segment, not a loop back to this one
    let other_probe = loop_guard_test_helpers::build_probe_bytes(0xFFFF_0000);
    let parsed = EthernetFrame::parse(&other_probe).unwrap();

    // Also pins down a known limitation as intentional, not a bug: a
    // peer switch's probe is recognized-and-swallowed (Ok(vec![])), not
    // flooded onward — so a loop spanning *two* vlan-rs switches never
    // gets the probe back to its originator. Only a direct self-loop on
    // one switch is detectable today; see loop_guard's module docs.
    let deliveries = switch.forward(PORT1, &parsed, now()).unwrap();
    assert!(deliveries.is_empty());
    assert!(!switch.is_blocked(PORT1));
}

#[test]
fn a_zero_padded_probe_is_still_recognized() {
    // Real hardware pads any frame under the 802.3 60-byte minimum before
    // it goes out on the wire; `build_loop_probe` already accounts for
    // this by padding to that minimum itself, but a peer that padded
    // *further* (or a NIC that adds trailing garbage) must not defeat
    // detection either — only the leading magic+id bytes matter.
    let mut switch = two_vlans_with_probe(0xABCD_1234);
    let probe = switch.build_loop_probe();
    let mut padded = probe.clone();
    padded.extend_from_slice(&[0u8; 16]);
    let parsed = EthernetFrame::parse(&padded).unwrap();

    let deliveries = switch.forward(PORT1, &parsed, now()).unwrap();
    assert!(deliveries.is_empty());
    assert!(switch.is_blocked(PORT1));
}

#[test]
fn two_switches_get_different_probe_ids() {
    let a = Switch::new();
    let b = Switch::new();
    assert_ne!(a.probe_id(), b.probe_id());
}

#[test]
fn unicast_to_a_mac_learned_on_a_since_blocked_port_does_not_egress_it() {
    let mut switch = two_vlans_with_probe(1);
    // host A is learned on PORT1 while it's still healthy...
    switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), now())
        .unwrap();
    // ...then PORT1 gets blocked (as the loop guard would do), but the
    // stale MAC-table entry must not survive it, and even if it did,
    // encode_for_egress must still refuse to send out a blocked port.
    switch.block_port(PORT1);

    let deliveries = switch
        .forward(PORT2, &frame(HOST_A, HOST_B), now())
        .unwrap();
    assert!(
        deliveries.is_empty(),
        "a blocked port must receive no traffic, unicast included"
    );
}

#[test]
fn a_blocked_port_rejects_forward_calls() {
    let mut switch = two_vlans_with_probe(1);
    switch.block_port(PORT1);

    let err = switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), now())
        .unwrap_err();
    assert_eq!(err, SwitchError::PortBlocked(PORT1));
}

#[test]
fn a_blocked_port_is_excluded_from_flooding() {
    let mut switch = two_vlans_with_probe(1);
    switch.block_port(PORT2);

    // port 1 is the only *unblocked* other member of vlan 10 — but it's
    // also the ingress port here, so a broadcast from a third, freshly
    // added port must reach neither the ingress nor the blocked port
    switch.add_port(PORT3, PortMode::access(10).unwrap());
    let deliveries = switch
        .forward(PORT3, &frame(BROADCAST, HOST_C), now())
        .unwrap();
    assert_eq!(ports_of(&deliveries), vec![PORT1]);
}

#[test]
fn unblock_port_restores_normal_forwarding() {
    let mut switch = two_vlans_with_probe(1);
    switch.block_port(PORT1);
    switch.unblock_port(PORT1);

    let deliveries = switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), now())
        .unwrap();
    assert_eq!(ports_of(&deliveries), vec![PORT2]);
}

#[test]
fn re_adding_a_port_clears_a_block() {
    let mut switch = two_vlans_with_probe(1);
    switch.block_port(PORT1);

    switch.add_port(PORT1, PortMode::access(10).unwrap());
    assert!(!switch.is_blocked(PORT1));
}

#[test]
fn a_probe_touches_no_counters() {
    let mut switch = two_vlans_with_probe(0xABCD_1234);
    let probe = switch.build_loop_probe();
    let parsed = EthernetFrame::parse(&probe).unwrap();

    switch.forward(PORT1, &parsed, now()).unwrap();
    assert_eq!(switch.port_counters(PORT1), Counters::default());
    assert_eq!(switch.vlan_counters(10), Counters::default());
}

/// A minimal re-implementation of `Switch::build_loop_probe`'s wire format
/// — magic prefix, probe id, zero-padded to the 802.3 minimum frame size
/// — independent of the switch under test, so
/// `a_different_switchs_probe_does_not_block_anything` can build "some
/// other switch's" probe without exposing `loop_guard`'s internals
/// outside the crate. Keep in sync with `src/switch/loop_guard.rs`
/// (`PROBE_MAGIC`, `PROBE_PAYLOAD_LEN`), the actual source of truth for
/// this format — this copy silently drifts if that one changes.
mod loop_guard_test_helpers {
    use vlan_rs::frame::EthernetFrame;

    const PROBE_MAGIC: [u8; 4] = *b"VLPB";
    const PROBE_PAYLOAD_LEN: usize = 46;

    pub(super) fn build_probe_bytes(probe_id: u64) -> Vec<u8> {
        let mut src = [0u8; 6];
        src[0] = 0x02;
        src[1..6].copy_from_slice(&probe_id.to_be_bytes()[0..5]);
        let mut payload = [0u8; PROBE_PAYLOAD_LEN];
        payload[0..4].copy_from_slice(&PROBE_MAGIC);
        payload[4..12].copy_from_slice(&probe_id.to_be_bytes());
        let frame = EthernetFrame {
            dst: [0xFF; 6],
            src,
            tag: None,
            ethertype: 0x88B7,
            payload: &payload,
        };
        let mut bytes = Vec::new();
        frame.write_into(&mut bytes).unwrap();
        bytes
    }
}
