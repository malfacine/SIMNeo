use sim_core::{Channel, DeviceDraft, Manufacturer};
use sim_storage_import::{NewDevice, Store};

fn draft(name: &str, address: &str) -> DeviceDraft {
    DeviceDraft {
        serial: None,
        connection: sim_core::ConnectionMode::Address,
        name: name.into(),
        address: address.into(),
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
fn saves_uppercase_serials_and_reads_legacy_lowercase_without_changing_credentials() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("serial-case.db");
    let mut store = Store::open(&path).unwrap();
    let mut p2p = draft("Legacy", "test-Serial01");
    p2p.connection = sim_core::ConnectionMode::P2p;
    store
        .insert_batch(&[NewDevice::new(p2p, "MixedPassword".into())])
        .unwrap();
    let id = store.list().unwrap()[0].id;
    let raw = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        raw.query_row("SELECT address FROM devices WHERE id=?1", [id], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "TEST-SERIAL01"
    );
    raw.execute(
        "UPDATE devices SET address='test-serial01' WHERE id=?1",
        [id],
    )
    .unwrap();
    assert_eq!(store.list().unwrap()[0].config.address, "TEST-SERIAL01");
    assert_eq!(&*store.password(id).unwrap(), "MixedPassword");
    let mut dual = draft("Dual", "Camera.Example.org");
    dual.serial = Some("mixed-Serial02".into());
    store.update(id, &NewDevice::new(dual, "".into())).unwrap();
    let (host, serial) = raw
        .query_row(
            "SELECT address,serial FROM devices WHERE id=?1",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(host, "Camera.Example.org");
    assert_eq!(serial, "MIXED-SERIAL02");
    raw.execute("UPDATE devices SET serial=lower(serial) WHERE id=?1", [id])
        .unwrap();
    assert_eq!(
        store.list().unwrap()[0].config.serial.as_deref(),
        Some("MIXED-SERIAL02")
    );
    assert_eq!(&*store.password(id).unwrap(), "MixedPassword");
}

#[test]
fn editing_preserves_or_replaces_password_and_delete_removes_channels() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crud.db");
    let mut store = Store::open(&path).unwrap();
    store
        .insert_batch(&[NewDevice::new(draft("A", "192.0.2.1"), "old".into())])
        .unwrap();
    let id = store.list().unwrap()[0].id;
    store
        .update(id, &NewDevice::new(draft("Edited", "192.0.2.2"), "".into()))
        .unwrap();
    assert_eq!(&*store.password(id).unwrap(), "old");
    assert_eq!(store.list().unwrap()[0].config.name, "Edited");
    store
        .update(
            id,
            &NewDevice::new(draft("Edited", "192.0.2.2"), "new".into()),
        )
        .unwrap();
    assert_eq!(&*store.password(id).unwrap(), "new");
    store.delete(id).unwrap();
    assert!(store.list().unwrap().is_empty());
    drop(store);
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM channels", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn duplicate_edit_rolls_back_device_channels_and_password() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("crud.db")).unwrap();
    store
        .insert_batch(&[
            NewDevice::new(draft("A", "192.0.2.1"), "original".into()),
            NewDevice::new(draft("B", "192.0.2.2"), "other".into()),
        ])
        .unwrap();
    let id = store.list().unwrap()[0].id;
    assert!(
        store
            .update(
                id,
                &NewDevice::new(draft("Changed", "192.0.2.2"), "changed".into())
            )
            .is_err()
    );
    assert_eq!(&*store.password(id).unwrap(), "original");
    assert_eq!(store.list().unwrap()[0].config.name, "A");
}

#[test]
fn persists_devices_channels_and_protected_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let mut store = Store::open(&path).unwrap();
    let input = NewDevice::new(
        draft("Recepção", "192.0.2.1"),
        "synthetic-test-password".into(),
    );
    store.insert_batch(&[input]).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    let devices = store.list().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].config.channels[0].name, "Entrada");
    assert_eq!(
        &*store.password(devices[0].id).unwrap(),
        "synthetic-test-password"
    );
    let raw = std::fs::read(path).unwrap();
    assert!(!raw.windows(23).any(|x| x == b"synthetic-test-password"));
}

#[test]
fn duplicate_rolls_back_the_whole_batch() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("test.db")).unwrap();
    let one = NewDevice::new(draft("A", "192.0.2.1"), "".into());
    let two = NewDevice::new(draft("B", "192.0.2.1"), "".into());
    assert!(store.insert_batch(&[one, two]).is_err());
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn invalid_device_does_not_leave_partial_data() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("test.db")).unwrap();
    let one = NewDevice::new(draft("A", "192.0.2.1"), "".into());
    let mut bad = draft("B", "192.0.2.2");
    bad.port = 0;
    assert!(
        store
            .insert_batch(&[one, NewDevice::new(bad, "".into())])
            .is_err()
    );
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn preferences_persist_and_replace_without_changing_devices() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.db");
    let store = Store::open(&path).unwrap();
    assert!(store.setting("preferences").unwrap().is_none());
    store.set_setting("preferences", r#"{"gpu":true}"#).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.setting("preferences").unwrap().as_deref(),
        Some(r#"{"gpu":true}"#)
    );
    store
        .set_setting("preferences", r#"{"gpu":false}"#)
        .unwrap();
    assert!(store.list().unwrap().is_empty());
    assert_eq!(
        store.setting("preferences").unwrap().as_deref(),
        Some(r#"{"gpu":false}"#)
    );
}

#[test]
fn migrates_v3_and_edits_dual_connection_without_losing_credentials_or_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dual.db");
    let mut store = Store::open(&path).unwrap();
    store
        .insert_batch(&[NewDevice::new(draft("A", "192.0.2.1"), "synthetic".into())])
        .unwrap();
    store.set_setting("preferences", "fixture").unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("ALTER TABLE devices DROP COLUMN serial; PRAGMA user_version=3;")
        .unwrap();
    drop(conn);
    let mut store = Store::open(&path).unwrap();
    let d = store.list().unwrap().remove(0);
    assert!(d.config.serial.is_none());
    assert_eq!(
        store.setting("preferences").unwrap().as_deref(),
        Some("fixture")
    );
    let mut dual = d.config;
    dual.serial = Some("TESTSERIAL".into());
    store
        .update(d.id, &NewDevice::new(dual.clone(), String::new()))
        .unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.list().unwrap()[0].config.serial, dual.serial);
    assert_eq!(&*store.password(d.id).unwrap(), "synthetic");
    dual.serial = None;
    store
        .update(d.id, &NewDevice::new(dual, String::new()))
        .unwrap();
    assert!(store.list().unwrap()[0].config.serial.is_none());
}
