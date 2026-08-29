use vlan_rs::switch::{InvalidVlan, PortMode, PortModeError};

#[test]
fn access_accepts_the_assignable_range() {
    assert_eq!(PortMode::access(1), Ok(PortMode::Access { vlan: 1 }));
    assert_eq!(PortMode::access(4094), Ok(PortMode::Access { vlan: 4094 }));
}

#[test]
fn access_rejects_reserved_and_out_of_range_vids() {
    assert_eq!(PortMode::access(0), Err(InvalidVlan(0)));
    assert_eq!(PortMode::access(4095), Err(InvalidVlan(4095)));
    assert_eq!(PortMode::access(4096), Err(InvalidVlan(4096)));
}

#[test]
fn trunk_accepts_the_assignable_range() {
    assert!(PortMode::trunk(Some(1), [4094]).is_ok());
    assert!(PortMode::trunk(Some(4094), [1]).is_ok());
}

#[test]
fn trunk_rejects_reserved_and_out_of_range_vids() {
    assert_eq!(
        PortMode::trunk(Some(0), [10]),
        Err(PortModeError::InvalidVlan(0))
    );
    assert_eq!(
        PortMode::trunk(Some(4095), [10]),
        Err(PortModeError::InvalidVlan(4095))
    );
    assert_eq!(
        PortMode::trunk(Some(4096), [10]),
        Err(PortModeError::InvalidVlan(4096))
    );
    assert_eq!(
        PortMode::trunk(None, [4096]),
        Err(PortModeError::InvalidVlan(4096))
    );
    assert_eq!(
        PortMode::trunk(None, [0]),
        Err(PortModeError::InvalidVlan(0))
    );
    assert_eq!(
        PortMode::trunk(None, [4095]),
        Err(PortModeError::InvalidVlan(4095))
    );
}
