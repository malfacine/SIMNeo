use sim_core::{Channel, ConnectionMode, DeviceDraft, Manufacturer};
use sim_storage_import::{NewDevice, Store};

#[test]
fn persists_serial_mode_and_sync_adds_channels_without_replacing_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sync.db");
    let mut store = Store::open(&path).unwrap();
    store
        .insert_batch(&[NewDevice::new(
            DeviceDraft {
                serial: None,
                connection: ConnectionMode::P2p,
                name: "DVR fixture".into(),
                address: "TESTSERIAL".into(),
                port: 37777,
                username: "admin".into(),
                manufacturer: Manufacturer::Dahua,
                channels: vec![Channel {
                    number: 0,
                    name: "Nome personalizado".into(),
                }],
            },
            "test-only".into(),
        )])
        .unwrap();
    let id = store.list().unwrap()[0].id;
    store
        .sync_channels(
            id,
            &[
                Channel {
                    number: 0,
                    name: "Nome do DVR".into(),
                },
                Channel {
                    number: 1,
                    name: "Garagem".into(),
                },
            ],
        )
        .unwrap();
    drop(store);
    let devices = Store::open(&path).unwrap().list().unwrap();
    assert_eq!(devices[0].config.connection, ConnectionMode::P2p);
    assert_eq!(devices[0].config.channels[0].name, "Nome personalizado");
    assert_eq!(devices[0].config.channels[1].name, "Garagem");
}

#[test]
fn migrates_v1_without_losing_devices_or_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!("../src/schema.sql")).unwrap();
    db.execute(
        "INSERT INTO devices VALUES(1,'Legado','192.0.2.1',37777,'admin','dahua',?1)",
        [vec![1u8, 2, 3]],
    )
    .unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    let devices = store.list().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].config.connection, ConnectionMode::Address);
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT password_dpapi FROM devices WHERE id=1", [], |r| r
            .get::<_, Vec<
            u8,
        >>(
            0
        ))
        .unwrap(),
        vec![1, 2, 3]
    );
}

#[test]
fn invalid_sync_leaves_existing_channels_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("atomic.db");
    let mut store = Store::open(&path).unwrap();
    store
        .insert_batch(&[NewDevice::new(
            DeviceDraft {
                serial: None,
                connection: ConnectionMode::Address,
                name: "Fixture".into(),
                address: "192.0.2.10".into(),
                port: 37777,
                username: "admin".into(),
                manufacturer: Manufacturer::Dahua,
                channels: vec![Channel {
                    number: 0,
                    name: "Existente".into(),
                }],
            },
            "test".into(),
        )])
        .unwrap();
    let id = store.list().unwrap()[0].id;
    assert!(
        store
            .sync_channels(
                id,
                &[
                    Channel {
                        number: 1,
                        name: "Novo".into()
                    },
                    Channel {
                        number: 1,
                        name: "Duplicado".into()
                    }
                ]
            )
            .is_err()
    );
    let channels = store.list().unwrap().remove(0).config.channels;
    assert_eq!(
        channels,
        vec![Channel {
            number: 0,
            name: "Existente".into()
        }]
    );
}
