use vlan_rs::config::{Config, ConfigError};
use vlan_rs::switch::PortMode;

#[test]
fn parses_an_empty_config() {
    let specs = Config::from_toml_str("").unwrap().into_specs().unwrap();
    assert_eq!(specs, vec![]);
}

#[test]
fn parses_access_and_trunk_ports() {
    let toml = r#"
        [[port]]
        name = "tap0"
        mode = "access"
        vlan = 10

        [[port]]
        name = "tap1"
        mode = "trunk"
        native = 10
        allowed = [10, 20]
    "#;
    let specs = Config::from_toml_str(toml).unwrap().into_specs().unwrap();
    assert_eq!(
        specs,
        vec![
            ("tap0".to_owned(), PortMode::access(10).unwrap()),
            (
                "tap1".to_owned(),
                PortMode::trunk(Some(10), [10, 20]).unwrap()
            ),
        ]
    );
}

#[test]
fn trunk_native_and_allowed_are_optional() {
    let toml = r#"
        [[port]]
        name = "tap0"
        mode = "trunk"
        allowed = [10, 20]
    "#;
    let specs = Config::from_toml_str(toml).unwrap().into_specs().unwrap();
    assert_eq!(
        specs,
        vec![("tap0".to_owned(), PortMode::trunk(None, [10, 20]).unwrap())]
    );
}

#[test]
fn rejects_malformed_toml() {
    let err = Config::from_toml_str("this is not toml [[[").unwrap_err();
    assert!(matches!(err, ConfigError::Toml(_)));
}

#[test]
fn rejects_an_unknown_mode() {
    let toml = r#"
        [[port]]
        name = "tap0"
        mode = "bridge"
    "#;
    assert!(Config::from_toml_str(toml).is_err());
}

#[test]
fn rejects_an_out_of_range_vlan() {
    let toml = r#"
        [[port]]
        name = "tap0"
        mode = "access"
        vlan = 4095
    "#;
    let err = Config::from_toml_str(toml)
        .unwrap()
        .into_specs()
        .unwrap_err();
    assert!(matches!(err, ConfigError::InvalidPort { .. }));
}

#[test]
fn rejects_a_trunk_with_neither_native_nor_allowed() {
    let toml = r#"
        [[port]]
        name = "tap0"
        mode = "trunk"
    "#;
    let err = Config::from_toml_str(toml)
        .unwrap()
        .into_specs()
        .unwrap_err();
    assert!(matches!(err, ConfigError::InvalidPort { .. }));
}

#[test]
fn rejects_a_duplicate_port_name() {
    let toml = r#"
        [[port]]
        name = "tap0"
        mode = "access"
        vlan = 10

        [[port]]
        name = "tap0"
        mode = "access"
        vlan = 20
    "#;
    let err = Config::from_toml_str(toml)
        .unwrap()
        .into_specs()
        .unwrap_err();
    assert!(matches!(err, ConfigError::DuplicateName(name) if name == "tap0"));
}

#[test]
fn load_reports_a_clear_error_for_a_missing_file() {
    let err = Config::load("/nonexistent/path/to/vlan-rs-config-test.toml").unwrap_err();
    assert!(matches!(err, ConfigError::Io { .. }));
}
