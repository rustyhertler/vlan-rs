use vlan_rs::daemon::parse_port_specs;
use vlan_rs::switch::PortMode;

fn args(strs: &[&str]) -> std::vec::IntoIter<String> {
    strs.iter()
        .map(|s| (*s).to_owned())
        .collect::<Vec<_>>()
        .into_iter()
}

#[test]
fn parses_zero_or_more_access_specs() {
    assert_eq!(parse_port_specs(args(&[])).unwrap(), vec![]);
    assert_eq!(
        parse_port_specs(args(&["tap0:10", "tap1:10", "tap2:20"])).unwrap(),
        vec![
            ("tap0".to_owned(), PortMode::access(10).unwrap()),
            ("tap1".to_owned(), PortMode::access(10).unwrap()),
            ("tap2".to_owned(), PortMode::access(20).unwrap()),
        ]
    );
}

#[test]
fn parses_a_trunk_spec_with_native_and_allowed() {
    assert_eq!(
        parse_port_specs(args(&["tap0:trunk:10:10,20,30"])).unwrap(),
        vec![(
            "tap0".to_owned(),
            PortMode::trunk(Some(10), [10, 20, 30]).unwrap()
        )]
    );
}

#[test]
fn parses_a_trunk_spec_with_no_native() {
    assert_eq!(
        parse_port_specs(args(&["tap0:trunk:-:10,20"])).unwrap(),
        vec![("tap0".to_owned(), PortMode::trunk(None, [10, 20]).unwrap())]
    );
}

#[test]
fn parses_an_untagged_only_trunk() {
    assert_eq!(
        parse_port_specs(args(&["tap0:trunk:10:"])).unwrap(),
        vec![("tap0".to_owned(), PortMode::trunk(Some(10), []).unwrap())]
    );
}

#[test]
fn rejects_a_trunk_with_neither_native_nor_allowed() {
    assert!(parse_port_specs(args(&["tap0:trunk:-:"])).is_err());
}

#[test]
fn rejects_an_empty_field_in_the_allowed_list() {
    // a stray or trailing comma is a likely typo (e.g. a deleted VLAN id),
    // not the same thing as an intentionally empty list
    assert!(parse_port_specs(args(&["tap0:trunk:-:10,,20"])).is_err());
    assert!(parse_port_specs(args(&["tap0:trunk:-:10,20,"])).is_err());
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
fn rejects_an_out_of_range_vlan_id() {
    assert!(parse_port_specs(args(&["tap0:0"])).is_err());
    assert!(parse_port_specs(args(&["tap0:4095"])).is_err());
    assert!(parse_port_specs(args(&["tap0:4096"])).is_err());
    assert!(parse_port_specs(args(&["tap0:trunk:0:10"])).is_err());
    assert!(parse_port_specs(args(&["tap0:trunk:-:0,10"])).is_err());
    assert!(parse_port_specs(args(&["tap0:trunk:4096:10"])).is_err());
    assert!(parse_port_specs(args(&["tap0:trunk:-:4096"])).is_err());
}

#[test]
fn accepts_assignable_vlan_id_bounds() {
    assert_eq!(
        parse_port_specs(args(&["tap0:1", "tap1:4094"])).unwrap(),
        vec![
            ("tap0".to_owned(), PortMode::access(1).unwrap()),
            ("tap1".to_owned(), PortMode::access(4094).unwrap()),
        ]
    );
}

#[test]
fn rejects_a_duplicate_tap_name() {
    // even across different VLANs — two ports on the same physical
    // interface would let the switch flood a frame back out where it
    // came from
    assert!(parse_port_specs(args(&["tap0:10", "tap0:20"])).is_err());
}
