use vlan_rs::frame::EthernetFrame;
use vlan_rs::switch::{BROADCAST, Forward, PortId, Switch, SwitchError};

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

/// Ports 1 & 2 in VLAN 10, port 3 alone in VLAN 20.
fn two_vlans() -> Switch {
    let mut switch = Switch::new();
    switch.add_port(PORT1, 10);
    switch.add_port(PORT2, 10);
    switch.add_port(PORT3, 20);
    switch
}

#[test]
fn floods_broadcast_only_within_ingress_vlan() {
    let mut switch = two_vlans();
    let decision = switch.forward(PORT1, &frame(BROADCAST, HOST_A)).unwrap();
    assert_eq!(decision, Forward::Flood(vec![PORT2]));
}

#[test]
fn floods_unknown_unicast_only_within_ingress_vlan() {
    let mut switch = two_vlans();
    // nobody has been learned yet, so an unknown dst still just floods vlan 10
    let decision = switch.forward(PORT1, &frame(HOST_B, HOST_A)).unwrap();
    assert_eq!(decision, Forward::Flood(vec![PORT2]));
}

#[test]
fn delivers_unicast_once_destination_is_learned() {
    let mut switch = two_vlans();
    // host B speaks first, so the switch learns it's behind port 2
    switch.forward(PORT2, &frame(BROADCAST, HOST_B)).unwrap();

    let decision = switch.forward(PORT1, &frame(HOST_B, HOST_A)).unwrap();
    assert_eq!(decision, Forward::Unicast(PORT2));
}

#[test]
fn same_mac_in_different_vlans_never_crosses_vlans() {
    let mut switch = two_vlans();
    // host C, on the vlan-20 port, is learned there
    switch.forward(PORT3, &frame(BROADCAST, HOST_C)).unwrap();

    // vlan 10 has never seen host C's MAC — the (vlan, mac) key keeps it
    // that way — so a vlan-10 port targeting it must still flood *within
    // vlan 10*, and can never resolve to port 3.
    let decision = switch.forward(PORT1, &frame(HOST_C, HOST_A)).unwrap();
    assert_eq!(decision, Forward::Flood(vec![PORT2]));
}

#[test]
fn drops_when_destination_learned_on_ingress_port() {
    let mut switch = two_vlans();
    switch.forward(PORT1, &frame(BROADCAST, HOST_A)).unwrap();

    let decision = switch.forward(PORT1, &frame(HOST_A, HOST_A)).unwrap();
    assert_eq!(decision, Forward::Drop);
}

#[test]
fn rejects_unknown_ingress_port() {
    let mut switch = two_vlans();
    let err = switch
        .forward(PortId(99), &frame(BROADCAST, HOST_A))
        .unwrap_err();
    assert_eq!(err, SwitchError::UnknownPort(PortId(99)));
}

#[test]
fn reassigning_a_port_purges_its_stale_routes() {
    let mut switch = two_vlans();
    // host A is learned behind port 1, vlan 10
    switch.forward(PORT1, &frame(BROADCAST, HOST_A)).unwrap();
    assert_eq!(
        switch.forward(PORT2, &frame(HOST_A, HOST_B)).unwrap(),
        Forward::Unicast(PORT1)
    );

    // port 1 gets re-provisioned into vlan 20 — its old vlan-10 route to
    // host A must not survive, or vlan-10 traffic could still reach it
    switch.add_port(PORT1, 20);

    let decision = switch.forward(PORT2, &frame(HOST_A, HOST_B)).unwrap();
    assert_eq!(
        decision,
        Forward::Flood(vec![]),
        "port 1 left vlan 10, so vlan 10 has no other member to flood to \
         — and it must never resolve to port 1 again"
    );
}

#[test]
fn removed_port_is_unknown_and_drops_out_of_flooding() {
    let mut switch = two_vlans();
    switch.forward(PORT1, &frame(BROADCAST, HOST_A)).unwrap();

    switch.remove_port(PORT1);

    let err = switch
        .forward(PORT1, &frame(BROADCAST, HOST_B))
        .unwrap_err();
    assert_eq!(err, SwitchError::UnknownPort(PORT1));

    // vlan 10's only remaining member is port 2, so a flood from it has
    // nowhere left to go
    let decision = switch.forward(PORT2, &frame(BROADCAST, HOST_B)).unwrap();
    assert_eq!(decision, Forward::Flood(vec![]));
}

#[test]
fn never_learns_a_multicast_or_broadcast_source() {
    let mut switch = two_vlans();
    // a frame claiming to be *from* a multicast address is malformed —
    // the switch must not treat it as a real host behind port 1
    switch
        .forward(PORT1, &frame(HOST_A, MULTICAST_SRC))
        .unwrap();

    // so a later frame *to* that same multicast address still floods,
    // rather than resolving to a bogus single unicast port
    let decision = switch
        .forward(PORT2, &frame(MULTICAST_SRC, HOST_B))
        .unwrap();
    assert_eq!(decision, Forward::Flood(vec![PORT1]));
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
    match switch.forward(PORT1, &parsed).unwrap() {
        Forward::Flood(targets) => {
            for (id, tx) in &senders {
                if targets.contains(id) {
                    tx.send(wire.clone()).unwrap();
                }
            }
        }
        other => panic!("expected a flood, got {other:?}"),
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
