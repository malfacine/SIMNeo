use serde_json::{Value, json};
use sim_proto_dvrip::{Session, sofia_hash};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn request(stream: &mut tokio::net::TcpStream) -> (u16, u32, Value) {
    let mut h = [0; 20];
    stream.read_exact(&mut h).await.unwrap();
    assert_eq!(h[0], 0xff);
    let len = u32::from_le_bytes(h[16..20].try_into().unwrap()) as usize;
    let mut body = vec![0; len];
    stream.read_exact(&mut body).await.unwrap();
    assert_eq!(&body[len - 2..], b"\n\0");
    (
        u16::from_le_bytes(h[14..16].try_into().unwrap()),
        u32::from_le_bytes(h[8..12].try_into().unwrap()),
        serde_json::from_slice(&body[..len - 2]).unwrap(),
    )
}
async fn reply(stream: &mut tokio::net::TcpStream, command: u16, id: u32, value: Value) {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.extend_from_slice(b"\n\0");
    let mut h = [0; 20];
    h[0] = 0xff;
    h[4..8].copy_from_slice(&7u32.to_le_bytes());
    h[8..12].copy_from_slice(&id.to_le_bytes());
    h[14..16].copy_from_slice(&(command + 1).to_le_bytes());
    h[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    for chunk in h.chunks(3) {
        stream.write_all(chunk).await.unwrap();
    }
    stream.write_all(&bytes).await.unwrap();
}

#[test]
fn sofia_hash_matches_external_vector() {
    assert_eq!(sofia_hash("admin"), "6QNMIQGe");
}

#[tokio::test]
async fn login_discovers_channels_and_keeps_session_alive() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (cmd, id, payload) = request(&mut s).await;
        assert_eq!(cmd, 1000);
        assert_eq!(payload["PassWord"], "6QNMIQGe");
        reply(
            &mut s,
            cmd,
            id,
            json!({"Ret":100,"SessionID":"0x00000007","AliveInterval":20,"ChannelNum":2}),
        )
        .await;
        let (cmd, id, _) = request(&mut s).await;
        assert_eq!(cmd, 1042);
        reply(
            &mut s,
            cmd,
            id,
            json!({"Ret":100,"ChannelTitle":["Entrada","Garagem"]}),
        )
        .await;
        let (cmd, id, _) = request(&mut s).await;
        assert_eq!(cmd, 1006);
        reply(&mut s, cmd, id, json!({"Ret":100})).await;
        let (cmd, id, _) = request(&mut s).await;
        assert_eq!(cmd, 1002);
        reply(&mut s, cmd, id, json!({"Ret":100})).await;
    });
    let mut session = Session::connect(addr, "admin", "admin").await.unwrap();
    assert_eq!(session.channels().await.unwrap()[1].name, "Garagem");
    session.keep_alive().await.unwrap();
    session.logout().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn authentication_failure_is_not_reported_as_online() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (cmd, id, _) = request(&mut s).await;
        reply(&mut s, cmd, id, json!({"Ret":203})).await;
    });
    assert!(Session::connect(addr, "admin", "invalid").await.is_err());
    server.await.unwrap();
}
