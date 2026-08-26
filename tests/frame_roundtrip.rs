use vlan_rs::frame::{Dot1qTag, EthernetFrame, ParseError};

/// Hand-built on the wire: broadcast dst, PCP=5/DEI=0/VID=42, EtherType
/// 0x88B5 (IEEE local-experimental — deliberately not IPv4/ARP, so the
/// etherparse cross-check below doesn't need a well-formed IP payload).
fn hand_built_tagged_frame() -> Vec<u8> {
    vec![
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // dst: broadcast
        0x02, 0x00, 0x00, 0x00, 0x00, 0x01, // src
        0x81, 0x00, // TPID: 802.1Q
        0xA0, 0x2A, // TCI: pcp=5, dei=0, vid=42
        0x88, 0xB5, // EtherType
        0x68, 0x69, // payload: "hi"
    ]
}

#[test]
fn parses_hand_built_tagged_frame() {
    let bytes = hand_built_tagged_frame();
    let frame = EthernetFrame::parse(&bytes).unwrap();

    assert_eq!(frame.dst, [0xFF; 6]);
    assert_eq!(frame.src, [0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
    let tag = frame.tag.expect("frame should be tagged");
    assert_eq!(tag.pcp, 5);
    assert!(!tag.dei);
    assert_eq!(tag.vid, 42);
    assert_eq!(frame.ethertype, 0x88B5);
    assert_eq!(frame.payload, b"hi");
}

#[test]
fn roundtrips_tagged_frame() {
    let original = EthernetFrame {
        dst: [0xAA; 6],
        src: [0xBB; 6],
        tag: Some(Dot1qTag {
            pcp: 3,
            dei: true,
            vid: 100,
        }),
        ethertype: 0x0800,
        payload: &[1, 2, 3, 4],
    };
    let mut bytes = Vec::new();
    original.write_into(&mut bytes);

    let parsed = EthernetFrame::parse(&bytes).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn roundtrips_untagged_frame() {
    let original = EthernetFrame {
        dst: [0x11; 6],
        src: [0x22; 6],
        tag: None,
        ethertype: 0x0806,
        payload: &[9, 9, 9],
    };
    let mut bytes = Vec::new();
    original.write_into(&mut bytes);
    assert_eq!(bytes.len(), 14 + original.payload.len());

    let parsed = EthernetFrame::parse(&bytes).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn rejects_frame_shorter_than_ethernet_header() {
    let bytes = [0u8; 13];
    assert_eq!(
        EthernetFrame::parse(&bytes),
        Err(ParseError::TooShort { len: 13 })
    );
}

#[test]
fn rejects_tagged_frame_truncated_before_ethertype() {
    let mut bytes = vec![0u8; 12]; // dst + src
    bytes.extend_from_slice(&0x8100u16.to_be_bytes()); // TPID
    bytes.extend_from_slice(&[0x00, 0x00]); // TCI, then nothing — no EtherType
    let len = bytes.len();

    assert_eq!(
        EthernetFrame::parse(&bytes),
        Err(ParseError::TruncatedTag { len })
    );
}

/// Cross-checks the hand-rolled parser against `etherparse` on the same
/// bytes — a second, independently-written implementation agreeing with
/// ours is stronger evidence than our own round-trip tests alone.
#[test]
fn cross_check_against_etherparse() {
    use etherparse::{LinkExtSlice, LinkSlice, SlicedPacket};

    let bytes = hand_built_tagged_frame();
    let ours = EthernetFrame::parse(&bytes).unwrap();

    let sliced = SlicedPacket::from_ethernet(&bytes).expect("etherparse should accept this frame");
    let LinkSlice::Ethernet2(eth) = sliced.link.expect("link header present") else {
        panic!("expected an Ethernet II link header");
    };
    assert_eq!(ours.dst, eth.destination());
    assert_eq!(ours.src, eth.source());

    assert_eq!(sliced.link_exts.len(), 1);
    let LinkExtSlice::Vlan(vlan) = &sliced.link_exts[0] else {
        panic!("expected a VLAN link extension");
    };
    let tag = ours.tag.expect("frame should be tagged");
    assert_eq!(tag.pcp, vlan.priority_code_point().value());
    assert_eq!(tag.dei, vlan.drop_eligible_indicator());
    assert_eq!(tag.vid, vlan.vlan_identifier().value());
    assert_eq!(ours.ethertype, vlan.ether_type().0);
    assert_eq!(ours.payload, vlan.payload_slice());
}
