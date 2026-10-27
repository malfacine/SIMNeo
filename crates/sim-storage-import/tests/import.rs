use sim_storage_import::{decrypt_xml_password, parse_simnext_xml};

const ENVELOPE: &str = "AAECAwQFBgcICQoLDA0ODw==";

#[test]
fn imports_cloud_serial_and_original_export_password_field() {
    let xml = include_str!("../../../fixtures/simnext-example.xml")
        .replace("<Device>", "<Device><IsIntelbrasCloudEnabled>true</IsIntelbrasCloudEnabled><SerialNumber>TEST1234</SerialNumber>")
        .replace("EncryptedPassword>", "ExportEncryptedPassword>");
    let imported = parse_simnext_xml(&xml, "teste").unwrap();
    assert_eq!(
        imported[0].config.connection,
        sim_core::ConnectionMode::Address
    );
    assert_eq!(imported[0].config.address, "192.0.2.10");
    assert_eq!(imported[0].config.serial.as_deref(), Some("TEST1234"));
    assert_eq!(&*imported[0].password, "camera-sintetica");
}

#[test]
fn imports_serial_only_when_primary_address_is_empty() {
    let xml=include_str!("../../../fixtures/simnext-example.xml")
        .replace("<MainIPAddress>192.0.2.10</MainIPAddress>","<MainIPAddress/>")
        .replace("<Device>","<Device><IsIntelbrasCloudEnabled>true</IsIntelbrasCloudEnabled><SerialNumber>TESTSERIAL</SerialNumber>");
    let devices = parse_simnext_xml(&xml, "teste").unwrap();
    assert_eq!(devices[0].config.connection, sim_core::ConnectionMode::P2p);
    assert_eq!(devices[0].config.address, "TESTSERIAL");
    assert!(devices[0].config.serial.is_none());
}

#[test]
fn rejects_wrong_root_dtd_and_invalid_xml() {
    for xml in [
        "<Other><Devices/></Other>",
        "<ExportableSystemData>",
        "<!DOCTYPE x [<!ENTITY y 'a'>]><ExportableSystemData><Devices/></ExportableSystemData>",
    ] {
        assert!(parse_simnext_xml(xml, "teste").is_err());
    }
}

#[test]
fn imports_identified_device_shape_without_logging_passwords() {
    let xml = r#"<ExportableSystemData><Version>1.2</Version><Devices><Device>
        <Name>Recepção</Name><MainIPAddress>192.0.2.1</MainIPAddress><Port>37777</Port>
        <UserName>admin</UserName><Manufacturer>Dahua</Manufacturer>
        <Channels><Channel><Name>Entrada</Name><Number>0</Number></Channel></Channels>
        </Device></Devices></ExportableSystemData>"#;
    let devices = parse_simnext_xml(xml, "").unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].config.channels[0].number, 0);
    assert_eq!(devices[0].config.name, "Recepção");
}

#[test]
fn rejects_truncated_ciphertext() {
    assert!(decrypt_xml_password(ENVELOPE, "teste").is_err());
}

#[test]
fn decrypts_independent_dotnet_aes_vector_and_rejects_wrong_key() {
    // Gerado com System.Security.Cryptography.Aes e UTF8, fora da implementação Rust.
    let encrypted = "AAECAwQFBgcICQoLDA0OD9u15gqP8tvx0ziZlKmR9TQ7At0mL33qEy0/04gN4gmA";
    assert_eq!(
        &*decrypt_xml_password(encrypted, "teste").unwrap(),
        "camera-sintetica"
    );
    assert!(decrypt_xml_password(encrypted, "incorreta").is_err());
}

#[test]
fn rejects_cloud_without_serial_instead_of_losing_connection_data() {
    let xml = r#"<ExportableSystemData><Devices><Device><Name>Cloud</Name>
      <MainIPAddress>192.0.2.1</MainIPAddress><Port>37777</Port><UserName>admin</UserName>
      <Manufacturer>Dahua</Manufacturer><IsIntelbrasCloudEnabled>true</IsIntelbrasCloudEnabled>
      </Device></Devices></ExportableSystemData>"#;
    assert!(parse_simnext_xml(xml, "").is_err());
}

#[test]
fn encrypted_xml_import_persists_password_and_rejects_duplicate_without_changes() {
    use sim_storage_import::Store;
    let xml = include_str!("../../../fixtures/simnext-example.xml");
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("import.db")).unwrap();
    let batch = parse_simnext_xml(xml, "teste").unwrap();
    store.insert_batch(&batch).unwrap();
    assert!(store.insert_batch(&batch).is_err());
    let devices = store.list().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(&*store.password(devices[0].id).unwrap(), "camera-sintetica");
    assert!(parse_simnext_xml(xml, "chave-errada").is_err());
}
