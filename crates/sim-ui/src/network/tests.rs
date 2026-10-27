use super::*;
use sim_storage_import::NewDevice;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};

#[test]
fn portable_service_profile_loads_without_windows_user_encryption() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("p2p-service.json");
    let profile = serde_json::json!({"server":"example.invalid","port":1250,"key":"synthetic-app","tunnel_password":"synthetic-transport","remote_ip":"127.0.0.1"});
    std::fs::write(&path, profile.to_string()).unwrap();
    let loaded = Coordinator::load(&path).unwrap();
    let embedded = Coordinator::configured(None, Some(&profile.to_string())).unwrap();
    assert_eq!(embedded.server, "example.invalid");
    assert!(Coordinator::configured(None, None).is_err());
    assert!(
        Coordinator::configured(
            Some(&folder.path().join("missing.json")),
            Some(&profile.to_string())
        )
        .is_err()
    );
    assert_eq!(loaded.server, "example.invalid");
    assert_eq!(loaded.port, 1250);
    assert_eq!(loaded.remote_ip, "127.0.0.1");
    std::fs::write(&path, "{}").unwrap();
    assert!(Coordinator::load(&path).is_err());
    let mut invalid = profile.clone();
    invalid["port"] = 0.into();
    std::fs::write(&path, invalid.to_string()).unwrap();
    assert!(Coordinator::load(&path).is_err());
    let mut invalid = profile;
    invalid["password"] = "synthetic-device".into();
    std::fs::write(&path, invalid.to_string()).unwrap();
    assert!(Coordinator::load(&path).is_err());
    std::fs::write(&path, "invalid json").unwrap();
    assert!(Coordinator::load(&path).is_err());
}

struct ControlServer {
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    closed: Arc<AtomicUsize>,
    accepted: Arc<AtomicUsize>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl ControlServer {
    fn new(refuse: bool) -> Self {
        Self::with_failure(refuse, false)
    }
    fn with_failure(refuse: bool, fail_first: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicUsize::new(0));
        let accepted = Arc::new(AtomicUsize::new(0));
        let (s, c, a) = (stop.clone(), closed.clone(), accepted.clone());
        let thread = std::thread::spawn(move || {
            let mut children = vec![];
            while !s.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        let connection = a.fetch_add(1, Ordering::AcqRel);
                        if fail_first && connection == 0 {
                            drop(socket);
                            c.fetch_add(1, Ordering::AcqRel);
                            continue;
                        }
                        let s = s.clone();
                        let c = c.clone();
                        children.push(std::thread::spawn(move || {
                            let _ = control(socket, &s, refuse);
                            c.fetch_add(1, Ordering::AcqRel);
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
            for child in children {
                child.join().unwrap();
            }
        });
        Self {
            address,
            stop,
            closed,
            accepted,
            thread: Some(thread),
        }
    }
}
impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn bytes(socket: &mut TcpStream, stop: &AtomicBool, data: &mut [u8]) -> std::io::Result<()> {
    let mut at = 0;
    while at < data.len() {
        if stop.load(Ordering::Acquire) {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        match socket.read(&mut data[at..]) {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => at += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
fn packet(socket: &mut TcpStream, kind: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut h = [0u8; 32];
    h[0] = kind;
    h[4..8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    socket.write_all(&h)?;
    socket.write_all(payload)
}
fn control(mut socket: TcpStream, stop: &AtomicBool, refuse: bool) -> std::io::Result<()> {
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut h = [0; 32];
    bytes(&mut socket, stop, &mut h)?;
    packet(&mut socket, 0xb0, b"Realm:fixture\r\nRandom:1234\r\n")?;
    bytes(&mut socket, stop, &mut h)?;
    let mut body = vec![0; u32::from_le_bytes(h[4..8].try_into().unwrap()) as usize];
    bytes(&mut socket, stop, &mut body)?;
    h = [0; 32];
    h[0] = 0xb0;
    h[8..10].copy_from_slice(if refuse { &[1, 4] } else { &[0, 8] });
    h[16..20].copy_from_slice(&7u32.to_le_bytes());
    socket.write_all(&h)?;
    if refuse {
        return Ok(());
    }
    loop {
        bytes(&mut socket, stop, &mut h)?;
        let len = u32::from_le_bytes(h[4..8].try_into().unwrap()) as usize;
        if len > 4096 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let mut payload = vec![0; len];
        bytes(&mut socket, stop, &mut payload)?;
        let request: serde_json::Value = serde_json::from_slice(&payload)?;
        let response = if request["method"] == "configManager.getConfig" {
            serde_json::json!({"id":request["id"],"result":true,"params":{"table":[{"Name":"A"},{"Name":"B"}]}})
        } else {
            serde_json::json!({"id":request["id"],"result":true})
        };
        packet(&mut socket, 0xb0, &serde_json::to_vec(&response)?)?;
    }
}
fn seed(db: &std::path::Path, servers: &[&ControlServer]) -> Vec<Device> {
    let mut store = Store::open(db).unwrap();
    let drafts: Vec<_> = servers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            NewDevice::new(
                sim_core::DeviceDraft {
                    serial: None,
                    connection: ConnectionMode::Address,
                    name: format!("Fixture {i}"),
                    address: "127.0.0.1".into(),
                    port: s.address.port(),
                    username: "fixture".into(),
                    manufacturer: Manufacturer::Dahua,
                    channels: vec![],
                },
                "synthetic".into(),
            )
        })
        .collect();
    store.insert_batch(&drafts).unwrap();
    store.list().unwrap()
}
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(4);
    while !predicate() {
        assert!(Instant::now() < until, "Prazo do fixture expirou");
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn two_dvrs_connect_independently_and_disconnect_does_not_close_neighbor() {
    let a = ControlServer::new(false);
    let b = ControlServer::new(false);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("fixtures.db");
    let devices = seed(&db, &[&a, &b]);
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = start(
        db,
        Settings::default(),
        tx,
        std::array::from_fn(|_| Default::default()),
    );
    assert!(handle.connect(devices[0].clone()));
    assert!(handle.connect(devices[1].clone()));
    let mut online = vec![];
    wait_until(|| {
        while let Ok(event) = rx.try_recv() {
            if let Event::Online { id, .. } = event {
                online.push(id);
            }
        }
        online.len() == 2
    });
    assert!(handle.disconnect(devices[0].id));
    wait_until(|| a.closed.load(Ordering::Acquire) == 1);
    assert_eq!(b.closed.load(Ordering::Acquire), 0);
    assert_eq!(b.accepted.load(Ordering::Acquire), 1);
    drop(handle);
    wait_until(|| b.closed.load(Ordering::Acquire) == 1);
}

#[test]
fn dual_connection_prefers_working_ip_without_loading_missing_sdk() {
    let server = ControlServer::new(false);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("dual.db");
    let mut device = seed(&db, &[&server]).remove(0);
    device.config.serial = Some("TESTSERIAL".into());
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = start(
        db,
        Settings::default(),
        tx,
        std::array::from_fn(|_| Default::default()),
    );
    assert!(handle.connect(device));
    wait_until(|| {
        rx.try_iter().any(|e|matches!(e,Event::Device{state:DeviceState::Online,message,..} if message.contains("por IP")))
    });
    assert_eq!(server.accepted.load(Ordering::Acquire), 1);
    drop(handle);
}

#[test]
fn probe_discovers_exact_channels_without_saving_or_opening_video() {
    let server = ControlServer::new(false);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("probe.db");
    let device = seed(&db, &[&server]).remove(0);
    let mut config = device.config;
    config.channels.clear();
    config.serial = Some("TESTSERIAL".into());
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = start(
        db.clone(),
        Settings::default(),
        tx,
        std::array::from_fn(|_| Default::default()),
    );
    assert!(handle.probe(42, NewDevice::new(config, "synthetic".into()), None));
    let mut found = None;
    wait_until(|| {
        for event in rx.try_iter() {
            if let Event::Probe { token, result } = event {
                assert_eq!(token, 42);
                found = Some(result.unwrap());
            }
        }
        found.is_some()
    });
    let (channels, message) = found.unwrap();
    assert_eq!(channels.len(), 2);
    assert_eq!(channels[0].name, "A");
    assert!(message.contains("IP"));
    wait_until(|| server.closed.load(Ordering::Acquire) == 1);
    assert_eq!(server.accepted.load(Ordering::Acquire), 1);
    let store = Store::open(&db).unwrap();
    assert_eq!(store.list().unwrap().len(), 1);
    assert!(store.list().unwrap()[0].config.serial.is_none());
    drop(handle);
}

#[test]
fn cancelled_probe_closes_connection_without_publishing_result() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cancel-probe.db");
    Store::open(&db).unwrap();
    let config = sim_core::DeviceDraft {
        serial: None,
        connection: ConnectionMode::Address,
        name: "Fixture".into(),
        address: "127.0.0.1".into(),
        port: endpoint.port(),
        username: "fixture".into(),
        manufacturer: Manufacturer::Dahua,
        channels: vec![],
    };
    let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = [0; 32];
        socket.read_exact(&mut bytes).unwrap();
        accepted_tx.send(()).unwrap();
        assert_eq!(socket.read(&mut bytes).unwrap(), 0);
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = start(
        db,
        Settings::default(),
        tx,
        std::array::from_fn(|_| Default::default()),
    );
    assert!(handle.probe(1, NewDevice::new(config, "synthetic".into()), None));
    accepted_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    handle.cancel_probe(1);
    thread.join().unwrap();
    assert!(!rx.try_iter().any(|e| matches!(e, Event::Probe { .. })));
    drop(handle);
}

#[test]
fn probe_uses_saved_password_for_edit_and_rejection_is_not_retried() {
    for refuse in [false, true] {
        let server = ControlServer::new(refuse);
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("edit-probe.db");
        let device = seed(&db, &[&server]).remove(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = start(
            db.clone(),
            Settings::default(),
            tx,
            std::array::from_fn(|_| Default::default()),
        );
        assert!(handle.probe(
            1,
            NewDevice::new(device.config, String::new()),
            Some(device.id)
        ));
        let mut found = None;
        wait_until(|| {
            for e in rx.try_iter() {
                if let Event::Probe { result, .. } = e {
                    found = Some(result);
                }
            }
            found.is_some()
        });
        assert_eq!(found.unwrap().is_err(), refuse);
        std::thread::sleep(Duration::from_millis(2100));
        assert_eq!(server.accepted.load(Ordering::Acquire), 1);
        assert_eq!(
            &*Store::open(&db).unwrap().password(device.id).unwrap(),
            "synthetic"
        );
        drop(handle);
    }
}

#[test]
fn dual_connection_tries_serial_after_ip_error_and_reports_missing_configuration() {
    let server = ControlServer::new(true);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("dual-error.db");
    let mut device = seed(&db, &[&server]).remove(0);
    device.config.serial = Some("TESTSERIAL".into());
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = start(
        db,
        Settings::default(),
        tx,
        std::array::from_fn(|_| Default::default()),
    );
    assert!(handle.connect(device));
    let mut attempted = false;
    let mut failed = false;
    wait_until(|| {
        for e in rx.try_iter() {
            if let Event::Device { state, message, .. } = e {
                attempted |= message.contains("tentando pelo serial");
                let expected = if EMBEDDED_P2P_SERVICE.is_some() {
                    "Configure o SDK P2P"
                } else {
                    "serviço P2P ausente"
                };
                failed |= state == DeviceState::Error && message.contains(expected);
            }
        }
        failed
    });
    assert!(attempted);
    std::thread::sleep(Duration::from_millis(2100));
    assert_eq!(server.accepted.load(Ordering::Acquire), 1);
    drop(handle);
}
#[test]
fn rejected_authentication_is_terminal_and_not_retried() {
    let server = ControlServer::new(true);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("auth.db");
    let device = seed(&db, &[&server]).remove(0);
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = start(
        db,
        Settings::default(),
        tx,
        std::array::from_fn(|_| Default::default()),
    );
    assert!(handle.connect(device));
    wait_until(|| {
        rx.try_iter().any(|e| {
            matches!(
                e,
                Event::Device {
                    state: DeviceState::Error,
                    ..
                }
            )
        })
    });
    std::thread::sleep(Duration::from_millis(2100));
    assert_eq!(server.accepted.load(Ordering::Acquire), 1);
    drop(handle);
}

#[test]
fn transient_control_failure_reconnects_only_that_dvr() {
    let a = ControlServer::with_failure(false, true);
    let b = ControlServer::new(false);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("reconnect.db");
    let devices = seed(&db, &[&a, &b]);
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = start(
        db,
        Settings::default(),
        tx,
        std::array::from_fn(|_| Default::default()),
    );
    for device in devices {
        assert!(handle.connect(device));
    }
    let mut online = std::collections::HashSet::new();
    let mut reconnecting = false;
    wait_until(|| {
        for event in rx.try_iter() {
            match event {
                Event::Online { id, .. } => {
                    online.insert(id);
                }
                Event::Device {
                    state: DeviceState::Reconnecting,
                    ..
                } => reconnecting = true,
                _ => {}
            }
        }
        online.len() == 2
    });
    assert!(reconnecting);
    assert_eq!(a.accepted.load(Ordering::Acquire), 2);
    assert_eq!(b.accepted.load(Ordering::Acquire), 1);
    assert_eq!(b.closed.load(Ordering::Acquire), 0);
    drop(handle);
}
#[test]
fn cancelling_pending_login_closes_socket_promptly() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cancel.db");
    let mut store = Store::open(&db).unwrap();
    store
        .insert_batch(&[NewDevice::new(
            sim_core::DeviceDraft {
                serial: None,
                connection: ConnectionMode::Address,
                name: "Pending".into(),
                address: "127.0.0.1".into(),
                port: endpoint.port(),
                username: "fixture".into(),
                manufacturer: Manufacturer::Dahua,
                channels: vec![],
            },
            "synthetic".into(),
        )])
        .unwrap();
    let device = store.list().unwrap().remove(0);
    drop(store);
    let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut h = [0; 32];
        socket.read_exact(&mut h).unwrap();
        accepted_tx.send(()).unwrap();
        let mut b = [0];
        assert_eq!(socket.read(&mut b).unwrap(), 0);
    });
    let (tx, _rx) = std::sync::mpsc::channel();
    let handle = start(
        db,
        Settings::default(),
        tx,
        std::array::from_fn(|_| Default::default()),
    );
    assert!(handle.connect(device));
    accepted_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let start = Instant::now();
    drop(handle);
    assert!(start.elapsed() < Duration::from_secs(2));
    server.join().unwrap();
}
