use super::*;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

struct Rtsp {
    endpoint: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    drop_first: Arc<AtomicBool>,
    connections: Arc<[AtomicUsize; 2]>,
    hd: Arc<AtomicUsize>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Rtsp {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let drop_first = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
        let hd = Arc::new(AtomicUsize::new(0));
        let (s, d, c, h) = (
            stop.clone(),
            drop_first.clone(),
            connections.clone(),
            hd.clone(),
        );
        let thread = std::thread::spawn(move || {
            let mut children = vec![];
            while !s.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        let (s, d, c, h) = (s.clone(), d.clone(), c.clone(), h.clone());
                        children.push(std::thread::spawn(move || {
                            if let Err(error) = serve(socket, &s, &d, &c, &h) {
                                eprintln!("RTSP fixture: {error}");
                            }
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
            endpoint,
            stop,
            drop_first,
            connections,
            hd,
            thread: Some(thread),
        }
    }
}
impl Drop for Rtsp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn read_request(socket: &mut TcpStream) -> std::io::Result<String> {
    let mut request = vec![];
    while !request.ends_with(b"\r\n\r\n") {
        let mut b = [0];
        socket.read_exact(&mut b)?;
        request.push(b[0]);
        if request.len() > 8192 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
    }
    Ok(String::from_utf8_lossy(&request).into_owned())
}
fn nals() -> Vec<Vec<u8>> {
    let data = include_bytes!("../../../../../fixtures/media/test-h264.h264");
    let mut starts = vec![];
    let mut i = 0;
    while i + 3 < data.len() {
        let prefix = if data[i..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if data[i..].starts_with(&[0, 0, 1]) {
            3
        } else {
            0
        };
        if prefix > 0 {
            starts.push((i, prefix));
            i += prefix;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, (at, prefix))| {
            data[at + prefix..starts.get(i + 1).map_or(data.len(), |v| v.0)].to_vec()
        })
        .collect()
}
fn rtp(
    socket: &mut TcpStream,
    body: &[u8],
    seq: &mut u16,
    time: u32,
    last: bool,
) -> std::io::Result<()> {
    let mut p = vec![0x80, 96 | if last { 0x80 } else { 0 }];
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&time.to_be_bytes());
    p.extend_from_slice(&1234u32.to_be_bytes());
    p.extend_from_slice(body);
    *seq = seq.wrapping_add(1);
    socket.write_all(&[b'$', 0, (p.len() >> 8) as u8, p.len() as u8])?;
    socket.write_all(&p)
}
fn serve(
    mut socket: TcpStream,
    stop: &AtomicBool,
    drop_first: &AtomicBool,
    connections: &[AtomicUsize; 2],
    hd: &AtomicUsize,
) -> std::io::Result<()> {
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(1)))?;
    let mut channel = 0;
    loop {
        let request = read_request(&mut socket)?;
        let method = request.split_whitespace().next().unwrap_or("");
        let cseq = request
            .lines()
            .find_map(|l| l.strip_prefix("CSeq: "))
            .unwrap_or("1")
            .trim();
        let mut extra = String::new();
        let mut body = String::new();
        match method {
            "OPTIONS" => extra = "Public: OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN\r\n".into(),
            "DESCRIBE" => {
                channel = if request.lines().next().unwrap_or("").contains("channel=2") {
                    1
                } else {
                    0
                };
                connections[channel].fetch_add(1, Ordering::AcqRel);
                if request.lines().next().unwrap_or("").contains("subtype=0") {
                    hd.fetch_add(1, Ordering::AcqRel);
                }
                body="v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=fixture\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=fmtp:96 packetization-mode=1\r\na=control:trackID=0\r\n".into();
                extra = format!(
                    "Content-Type: application/sdp\r\nContent-Length: {}\r\n",
                    body.len()
                );
            }
            "SETUP" => {
                extra =
                    "Transport: RTP/AVP/TCP;unicast;interleaved=0-1\r\nSession: fixture\r\n".into()
            }
            "PLAY" => extra = "Session: fixture\r\n".into(),
            _ => {}
        }
        write!(
            socket,
            "RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\n{extra}\r\n{body}"
        )?;
        if method == "PLAY" {
            break;
        }
    }
    let mut seq = 0u16;
    let mut time = 0u32;
    let units = nals();
    while !stop.load(Ordering::Acquire) {
        for nal in &units {
            if stop.load(Ordering::Acquire)
                || (channel == 0 && drop_first.swap(false, Ordering::AcqRel))
            {
                return Ok(());
            }
            let slice = matches!(nal[0] & 31, 1 | 5);
            if nal.len() < 1200 {
                rtp(&mut socket, nal, &mut seq, time, slice)?;
            } else {
                let chunks: Vec<_> = nal[1..].chunks(1100).collect();
                for (i, chunk) in chunks.iter().enumerate() {
                    let last = i + 1 == chunks.len();
                    let mut payload = vec![
                        (nal[0] & 0xe0) | 28,
                        (nal[0] & 31)
                            | if i == 0 {
                                0x80
                            } else if last {
                                0x40
                            } else {
                                0
                            },
                    ];
                    payload.extend_from_slice(chunk);
                    rtp(&mut socket, &payload, &mut seq, time, last && slice)?;
                }
            }
            if slice {
                time = time.wrapping_add(9000);
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    }
    Ok(())
}
async fn wait(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "RTSP fixture: prazo expirou");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[test]
fn a_dropped_stream_reconnects_without_stopping_neighbor_and_quality_replaces_only_its_player() {
    let server = Rtsp::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let device = Device {
            id: 1,
            config: sim_core::DeviceDraft {
                serial: None,
                connection: ConnectionMode::Address,
                name: "Fixture".into(),
                address: "127.0.0.1".into(),
                port: 37777,
                username: "fixture".into(),
                manufacturer: Manufacturer::Dahua,
                channels: vec![],
            },
        };
        let (events, received) = std::sync::mpsc::channel();
        let frames: Frames = std::array::from_fn(|_| Default::default());
        let ctx = Context {
            device: &device,
            password: "synthetic",
            events: &events,
            frames: frames.clone(),
            relay: None,
        };
        let mut wanted = [None; CAPACITY];
        for (slot, target) in wanted.iter_mut().enumerate().take(2) {
            *target = Some(VideoRequest {
                channel: slot as u16,
                generation: 1,
                quality: StreamQuality::Sd,
                fallback: false,
                gpu: false,
            });
        }
        let (tx, mut requests) = watch::channel(wanted);
        let scenario = async {
            wait(|| {
                while let Ok(Event::Video {
                    slot,
                    state,
                    message,
                    ..
                }) = received.try_recv()
                {
                    eprintln!("fixture slot {slot}: {state:?} {message}");
                }
                frames[..2].iter().all(|f| f.lock().unwrap().is_some())
            })
            .await;
            frames[1].lock().unwrap().take();
            server.drop_first.store(true, Ordering::Release);
            let mut recovering = false;
            wait(|| {
                while let Ok(e) = received.try_recv() {
                    if matches!(
                        e,
                        Event::Video {
                            slot: 0,
                            state: VideoState::Reconnecting,
                            ..
                        }
                    ) {
                        recovering = true;
                    }
                }
                recovering
            })
            .await;
            wait(|| frames[1].lock().unwrap().is_some()).await;
            assert_eq!(
                server.connections[0].load(Ordering::Acquire),
                1,
                "O vizinho deve receber frames antes da reconexão do canal que caiu"
            );
            assert_eq!(
                server.connections[1].load(Ordering::Acquire),
                1,
                "O vizinho não pode ser reiniciado pela falha de outro canal"
            );
            wait(|| {
                server.connections[0].load(Ordering::Acquire) >= 2
                    && frames[0].lock().unwrap().is_some()
            })
            .await;
            wanted[1].as_mut().unwrap().quality = StreamQuality::Hd;
            wanted[1].as_mut().unwrap().generation = 2;
            tx.send(wanted).unwrap();
            wait(|| {
                frames[1]
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|f| f.generation == 2)
            })
            .await;
            assert_eq!(server.connections[0].load(Ordering::Acquire), 2);
            assert_eq!(server.hd.load(Ordering::Acquire), 1);
            wanted[0] = None;
            tx.send(wanted).unwrap();
            frames[1].lock().unwrap().take();
            wait(|| frames[0].lock().unwrap().is_none() && frames[1].lock().unwrap().is_some())
                .await;
            tx.send([None; CAPACITY]).unwrap();
        };
        let (result, ()) = tokio::join!(
            drive(&ctx, server.endpoint, &[0, 1], &mut requests),
            scenario
        );
        result.unwrap();
    });
}
