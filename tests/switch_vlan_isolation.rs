use vlan_rs::frame::EthernetFrame;
use vlan_rs::switch::{Forward, PortId, Switch, SwitchError};

const HOST_A: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x01]; // port 1, vlan 10
const HOST_B: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x02]; // port 2, vlan 10
const HOST_C: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x03]; // port 3, vlan 20
const BROADCAST: [u8; 6] = [0xFF; 6];

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
