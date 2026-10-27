use sim_core::{Channel, DeviceDraft, Manufacturer};

fn device() -> DeviceDraft {
    DeviceDraft {
        serial: None,
        connection: sim_core::ConnectionMode::Address,
        name: "Recepção".into(),
        address: "192.0.2.10".into(),
        port: 37777,
        username: "admin".into(),
        manufacturer: Manufacturer::Dahua,
        channels: vec![Channel {
            number: 0,
            name: "Entrada".into(),
        }],
    }
}

#[test]
fn accepts_lan_or_ddns_and_zero_based_channels() {
    let mut draft = device();
    assert!(draft.validate().is_ok());
    draft.address = "dvr.example.org".into();
    assert!(draft.validate().is_ok());
}

#[test]
fn rejects_blank_fields_zero_port_and_duplicate_channels() {
    let mut draft = device();
    draft.name = "  ".into();
    assert!(draft.validate().is_err());
    draft = device();
    draft.port = 0;
    assert!(draft.validate().is_err());
    draft = device();
    draft.address = "http://camera/path".into();
    assert!(draft.validate().is_err());
    draft = device();
    draft.channels.push(draft.channels[0].clone());
    assert!(draft.validate().is_err());
}

#[test]
fn serial_mode_rejects_urls_and_accepts_synthetic_serial() {
    let mut draft = device();
    draft.connection = sim_core::ConnectionMode::P2p;
    draft.address = "TESTSERIAL01".into();
    assert!(draft.validate().is_ok());
    draft.address = "camera.example.org".into();
    assert!(draft.validate().is_err());
}

#[test]
fn accepts_dual_connection_and_rejects_incomplete_or_invalid_fields() {
    let mut d = device();
    d.serial = Some("TESTSERIAL01".into());
    assert!(d.validate().is_ok());
    for serial in ["", "has space", "https://invalid", "serial.example.org"] {
        d.serial = Some(serial.into());
        assert!(d.validate().is_err());
    }
    d.serial = Some("TESTSERIAL01".into());
    d.address.clear();
    assert!(d.validate().is_err());
}
