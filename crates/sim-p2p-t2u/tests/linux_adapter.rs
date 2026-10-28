#![cfg(target_os = "linux")]
use sim_p2p_t2u::Client;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

fn fixture(version: u32) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("test.so");
    let status = Command::new("cc")
        .args([
            "-std=c11", "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror",
        ])
        .arg(format!("-DABI={version}"))
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linux_api.c"))
        .arg("-o")
        .arg(&library)
        .status()
        .unwrap();
    assert!(status.success());
    (dir, library)
}
fn echo(endpoint: SocketAddr, message: &[u8]) -> TcpStream {
    let mut stream = TcpStream::connect(endpoint).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream.write_all(message).unwrap();
    let mut buffer = vec![0; message.len()];
    stream.read_exact(&mut buffer).unwrap();
    assert_eq!(message, &buffer);
    stream
}
async fn until_no_handles(library: &libloading::Library) {
    let started = Instant::now();
    let active = unsafe {
        library
            .get::<unsafe extern "C" fn() -> i32>(b"test_active\0")
            .unwrap()
    };
    while unsafe { active() } != 0 {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "handle nativo não foi fechado"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[test]
fn rejects_incompatible_abi_and_invalid_arguments() {
    let (_dir, library) = fixture(2);
    assert!(matches!(
        Client::load(&library, "localhost", 1, "test"),
        Err(sim_p2p_t2u::Error::UnsupportedLibrary)
    ));
    assert!(Client::load(&library, "localhost\0invalid", 1, "test").is_err());
}

#[tokio::test]
async fn isolates_connections_reopens_and_closes_only_its_own_tunnel() {
    let (_dir, library) = fixture(1);
    let observer = unsafe { libloading::Library::new(&library).unwrap() };
    let client = Client::load(&library, "localhost", 1, "test").unwrap();
    let first = client
        .tunnel(
            "test-serial",
            "password",
            "127.0.0.1",
            554,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    let second = client
        .tunnel(
            "test-serial",
            "password",
            "127.0.0.1",
            37777,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert!(first.endpoint().ip().is_loopback());
    assert_eq!(first.status().unwrap(), 1);
    let mut one = echo(first.endpoint(), b"RTSP connection one");
    let mut two = echo(first.endpoint(), b"independent connection two");
    let mut other = echo(second.endpoint(), b"control tunnel");
    one.write_all(b"still one").unwrap();
    let mut response = [0; 9];
    one.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"still one");
    two.write_all(b"still two").unwrap();
    two.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"still two");
    drop(one);
    drop(echo(first.endpoint(), b"replacement connection"));
    drop(first);
    let mut byte = [0];
    assert_eq!(two.read(&mut byte).unwrap(), 0);
    other.write_all(b"unaffected").unwrap();
    let mut response = [0; 10];
    other.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"unaffected");
    drop(other);
    drop(second);
    drop(client);
    until_no_handles(&observer).await;
}

#[tokio::test]
async fn cancelling_open_does_not_block_runtime_or_leak_native_handle() {
    let (_dir, library) = fixture(1);
    let observer = unsafe { libloading::Library::new(&library).unwrap() };
    let client = Client::load(&library, "slow", 1, "test").unwrap();
    let started = Instant::now();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(30),
            client.tunnel(
                "test-serial",
                "password",
                "127.0.0.1",
                554,
                Duration::from_secs(1)
            )
        )
        .await
        .is_err()
    );
    assert!(started.elapsed() < Duration::from_millis(150));
    drop(client);
    tokio::time::sleep(Duration::from_millis(300)).await;
    until_no_handles(&observer).await;
}
