use serde_json::{Value, json};
use sim_proto_dahua::{Session, WireMode};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn read(stream: &mut tokio::net::TcpStream) -> Value {
    let mut head = [0; 32];
    stream.read_exact(&mut head).await.unwrap();
    assert_eq!(&head[4..8], b"DHIP");
    let len = u32::from_le_bytes(head[16..20].try_into().unwrap());
    let mut bytes = vec![0; len as usize];
    stream.read_exact(&mut bytes).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
async fn reply(stream: &mut tokio::net::TcpStream, payload: Value) {
    let bytes = serde_json::to_vec(&payload).unwrap();
    let mut head = [0; 32];
    head[0] = 0x20;
    head[4..8].copy_from_slice(b"DHIP");
    head[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    head[24..28].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    // Fragmentação deliberada: TCP não preserva mensagens.
    for chunk in head.chunks(3) {
        stream.write_all(chunk).await.unwrap();
    }
    for chunk in bytes.chunks(5) {
        stream.write_all(chunk).await.unwrap();
    }
}

#[tokio::test]
async fn finds_recording_intervals_and_destroys_remote_search_on_success_or_rejection() {
    for case in 0..7 {
        let rejected = case == 1 || case == 5;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let login = read(&mut stream).await;
            reply(&mut stream,json!({"id":login["id"],"session":7,"result":false,"params":{"realm":"fixture","random":"1234","encryption":"Default"}})).await;
            let auth = read(&mut stream).await;
            reply(
                &mut stream,
                json!({"id":auth["id"],"session":7,"result":true}),
            )
            .await;
            let create = read(&mut stream).await;
            assert_eq!(create["method"], "mediaFileFind.factory.create");
            if case == 6 {
                reply(
                    &mut stream,
                    json!({"id":create["id"],"result":false,"error":{"code":285409284}}),
                )
                .await;
                return;
            }
            reply(&mut stream, json!({"id":create["id"],"result":42})).await;
            let find = read(&mut stream).await;
            assert_eq!(find["object"], 42);
            assert_eq!(find["params"]["condition"]["Channel"], 1);
            assert_eq!(find["params"]["condition"]["VideoStream"], "Main");
            assert_eq!(find["params"]["condition"]["Flags"], json!(["*"]));
            assert!(find["params"]["condition"].get("Stream").is_none());
            reply(
                &mut stream,
                json!({"id":find["id"],"result":!rejected,"error":{"code":if case==5 {285409284}else{123}}}),
            )
            .await;
            if !rejected {
                let next = read(&mut stream).await;
                assert_eq!(next["method"], "mediaFileFind.findNextFile");
                let params = match case {
                    2 => json!({"found":0}),
                    3 => json!({"found":0,"infos":null}),
                    4 => json!({"found":1,"infos":null}),
                    _ => {
                        json!({"found":1,"infos":[{"StartTime":"2026-10-26 10:00:00","EndTime":"2026-10-26 10:30:00","Flags":["Timing"]}]})
                    }
                };
                reply(
                    &mut stream,
                    json!({"id":next["id"],"result":true,"params":params}),
                )
                .await;
                if case == 0 {
                    // A short page does not terminate SDK pagination: read through found=0.
                    let next = read(&mut stream).await;
                    assert_eq!(next["method"], "mediaFileFind.findNextFile");
                    reply(
                        &mut stream,
                        json!({"id":next["id"],"result":true,"params":{"found":0}}),
                    )
                    .await;
                }
            }
            for method in ["mediaFileFind.close", "mediaFileFind.destroy"] {
                let close = read(&mut stream).await;
                assert_eq!(close["method"], method);
                assert_eq!(close["object"], 42);
                reply(&mut stream, json!({"id":close["id"],"result":true})).await;
            }
        });
        let mut session = Session::connect(address, "admin", "test-only", WireMode::Dhip)
            .await
            .unwrap();
        let (start, end) =
            sim_core::recording::period("2026-10-26 00:00:00", "2026-10-26 23:59:59").unwrap();
        let result = session.recordings(1, &start, &end, 0).await;
        if case == 6 {
            assert!(matches!(
                result,
                Err(sim_proto_dahua::Error::Rejected(285409284))
            ));
        } else if case == 5 {
            assert!(result.unwrap().is_empty());
        } else if rejected {
            assert!(matches!(result, Err(sim_proto_dahua::Error::Rejected(123))));
        } else if case == 4 {
            assert!(matches!(
                result,
                Err(sim_proto_dahua::Error::RecordingProtocol(_))
            ));
        } else if case == 2 || case == 3 {
            assert!(result.unwrap().is_empty());
        } else {
            let items = result.unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].kind, "Timing");
            assert_eq!(items[0].channel, 1);
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn challenge_login_channels_keepalive_and_logout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let login = read(&mut stream).await;
        assert_eq!(login["method"], "global.login");
        assert_eq!(login["params"]["password"], "");
        reply(&mut stream,json!({"id":login["id"],"session":7,"result":false,"params":{"realm":"fixture","random":"1234","encryption":"Default"}})).await;
        let auth = read(&mut stream).await;
        assert_eq!(
            auth["params"]["password"],
            "527F3F989AD9832FD14968DFC7208D70"
        );
        reply(
            &mut stream,
            json!({"id":auth["id"],"session":7,"result":true,"params":{"keepAliveInterval":20}}),
        )
        .await;
        let query = read(&mut stream).await;
        assert_eq!(query["method"], "configManager.getConfig");
        reply(&mut stream,json!({"id":query["id"],"result":true,"params":{"table":[{"Name":"Entrada"},{"Name":"Garagem"}]}})).await;
        let keep = read(&mut stream).await;
        assert_eq!(keep["method"], "global.keepAlive");
        reply(&mut stream, json!({"id":keep["id"],"result":true})).await;
        let logout = read(&mut stream).await;
        assert_eq!(logout["method"], "global.logout");
        assert!(logout["params"].is_null());
        reply(&mut stream, json!({"id":logout["id"],"result":true})).await;
    });
    let mut session = Session::connect(address, "admin", "test-only", WireMode::Dhip)
        .await
        .unwrap();
    let channels = session.channels().await.unwrap();
    assert_eq!(channels.len(), 2);
    assert_eq!(channels[1].name, "Garagem");
    session.keep_alive().await.unwrap();
    session.logout().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn rejects_oversize_response_before_allocating_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read(&mut stream).await;
        let mut head = [0; 32];
        head[0] = 0x20;
        head[4..8].copy_from_slice(b"DHIP");
        head[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        stream.write_all(&head).await.unwrap();
    });
    assert!(
        Session::connect(address, "admin", "test-only", WireMode::Dhip)
            .await
            .is_err()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn binary_login_uses_both_digests_and_closes_tcp_without_json_logout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut head = [0; 32];
        stream.read_exact(&mut head).await.unwrap();
        assert_eq!(&head[..2], &[0xa0, 1]);
        let body = b"Realm:fixture\r\nRandom:1234\r\n";
        head = [0; 32];
        head[0] = 0xb0;
        head[1] = 1;
        head[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
        stream.write_all(&head).await.unwrap();
        stream.write_all(body).await.unwrap();
        stream.read_exact(&mut head).await.unwrap();
        assert_eq!(&head[..2], &[0xa0, 5]);
        let mut body = vec![0; u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize];
        stream.read_exact(&mut body).await.unwrap();
        assert_eq!(&body[..39], b"admin&&527F3F989AD9832FD14968DFC7208D70");
        assert_eq!(body.len(), 71);
        head = [0; 32];
        head[0] = 0xb0;
        head[8..10].copy_from_slice(&[0, 8]);
        head[16..20].copy_from_slice(&7u32.to_le_bytes());
        stream.write_all(&head).await.unwrap();
        let mut byte = [0; 1];
        assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
    });
    let session = Session::connect(address, "admin", "test-only", WireMode::Legacy)
        .await
        .unwrap();
    session.logout().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn binary_login_refusal_is_an_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut head = [0; 32];
        stream.read_exact(&mut head).await.unwrap();
        head = [0; 32];
        head[0] = 0xb0;
        head[8..10].copy_from_slice(&[1, 4]);
        stream.write_all(&head).await.unwrap();
    });
    assert!(
        Session::connect(address, "admin", "invalid", WireMode::Legacy)
            .await
            .is_err()
    );
    server.await.unwrap();
}
