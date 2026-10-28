#![cfg(target_os = "linux")]

use sim_core::{Channel, ConnectionMode, DeviceDraft, Manufacturer};
use sim_storage_import::{NewDevice, Store};
use std::{os::unix::fs::PermissionsExt, process::Command};

fn device() -> NewDevice {
    NewDevice::new(
        DeviceDraft {
            serial: None,
            connection: ConnectionMode::Address,
            name: "Teste Linux".into(),
            address: "192.0.2.1".into(),
            port: 37777,
            username: "admin".into(),
            manufacturer: Manufacturer::Dahua,
            channels: vec![Channel {
                number: 0,
                name: "Canal".into(),
            }],
        },
        "senha sintética ç漢字".into(),
    )
}

#[test]
fn credential_process() {
    let Ok(mode) = std::env::var("SIMNEO_CREDENTIAL_TEST") else {
        return;
    };
    let path = std::path::PathBuf::from(std::env::var_os("SIMNEO_CREDENTIAL_DB").unwrap());
    let mut store = Store::open(&path).unwrap();
    match mode.as_str() {
        "write" => {
            store.insert_batch(&[device()]).unwrap();
        }
        "read" => {
            let id = store.list().unwrap()[0].id;
            assert_eq!(&*store.password(id).unwrap(), "senha sintética ç漢字");
        }
        "unavailable" | "missing-key" | "locked-key" => {
            let id = store.list().unwrap()[0].id;
            assert!(store.password(id).is_err());
            if mode != "missing-key" {
                assert!(store.update(id, &device()).is_err());
                assert!(store.insert_batch(&[device()]).is_err());
                assert_eq!(store.list().unwrap().len(), 1);
            }
        }
        _ => panic!("modo inválido"),
    }
}

#[test]
fn persists_across_processes_and_fails_closed_without_keyring() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db");
    let missing = dir.path().join("secret-tool");
    std::fs::write(&missing, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&missing, std::fs::Permissions::from_mode(0o700)).unwrap();
    let locked = dir.path().join("locked-bin");
    std::fs::create_dir(&locked).unwrap();
    let tool = locked.join("secret-tool");
    std::fs::write(&tool, "#!/bin/sh\ncase \"$1\" in\nlookup) exit 1;;\nsearch) printf '[locked item]';;\nstore) exit 0;;\nesac\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
    for mode in [
        "write",
        "read",
        "unavailable",
        "missing-key",
        "locked-key",
        "read",
    ] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "credential_process", "--nocapture"])
            .env("SIMNEO_CREDENTIAL_TEST", mode)
            .env("SIMNEO_CREDENTIAL_DB", &db);
        if mode == "unavailable" {
            command.env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent/simneo-test-bus",
            );
        }
        if mode == "missing-key" {
            command.env("PATH", dir.path());
        }
        if mode == "locked-key" {
            command.env("PATH", &locked);
        }
        assert!(command.status().unwrap().success(), "modo: {mode}");
    }
}
