use vlan_rs::daemon::parse_port_specs;

fn args(strs: &[&str]) -> std::vec::IntoIter<String> {
    strs.iter()
        .map(|s| (*s).to_owned())
        .collect::<Vec<_>>()
        .into_iter()
}

#[test]
fn parses_zero_or_more_name_vlan_pairs() {
    assert_eq!(parse_port_specs(args(&[])).unwrap(), vec![]);
    assert_eq!(
        parse_port_specs(args(&["tap0:10", "tap1:10", "tap2:20"])).unwrap(),
        vec![
            ("tap0".to_owned(), 10),
            ("tap1".to_owned(), 10),
            ("tap2".to_owned(), 20),
        ]
    );
}

#[test]
fn rejects_a_spec_with_no_colon() {
    assert!(parse_port_specs(args(&["tap0"])).is_err());
}

#[test]
fn rejects_a_non_numeric_vlan_id() {
    assert!(parse_port_specs(args(&["tap0:ten"])).is_err());
}

#[test]
fn rejects_a_duplicate_tap_name() {
    // even across different VLANs — two ports on the same physical
    // interface would let the switch flood a frame back out where it
    // came from
    assert!(parse_port_specs(args(&["tap0:10", "tap0:20"])).is_err());
}
