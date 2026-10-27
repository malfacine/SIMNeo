use sim_media::{Event, FrameSlot, Player};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn verify_clip(file: &str, codec: &str, size: (u32, u32)) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/media")
        .join(file);
    let frames: FrameSlot = Default::default();
    let player = Player::start(
        path.to_string_lossy().into_owned().into(),
        frames.clone(),
        42,
    );
    let until = Instant::now() + Duration::from_secs(5);
    let mut started = false;
    while Instant::now() < until {
        if let Some(Event::Started {
            codec: actual,
            width,
            height,
            ..
        }) = player.try_event()
        {
            assert_eq!(actual, codec);
            assert_eq!((width, height), size);
            started = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(started, "O decoder não produziu um frame no prazo.");
    drop(player);
    let frame = frames.lock().unwrap().take().unwrap();
    assert_eq!(frame.generation, 42);
    assert_eq!(frame.rgb.len(), (size.0 * size.1 * 3) as usize);
    assert_ne!(frame.rgb.iter().min(), frame.rgb.iter().max());
}

#[test]
fn decodes_h264_with_ffmpeg_in_process() {
    verify_clip("test-h264.mp4", "h264", (96, 64));
}
#[test]
fn decodes_h265_with_ffmpeg_in_process() {
    verify_clip("test-h265.mp4", "hevc", (96, 64));
}

#[test]
fn drains_delayed_hevc_frames_at_end_of_file() {
    verify_clip("test-h265-gpu.mp4", "hevc", (640, 360));
}

#[test]
#[ignore = "Requer GPU/driver D3D11VA no computador de teste"]
fn decodes_h264_and_h265_on_real_gpu() {
    for file in ["test-h264.mp4", "test-h265-gpu.mp4"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/media")
            .join(file);
        let frames: FrameSlot = Default::default();
        let player = Player::start_accelerated(
            path.to_string_lossy().into_owned().into(),
            frames.clone(),
            1,
            true,
        );
        let until = Instant::now() + Duration::from_secs(10);
        let mut backend = None;
        while Instant::now() < until {
            if let Some(Event::Started {
                backend: actual, ..
            }) = player.try_event()
            {
                backend = Some(actual);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(backend, Some("GPU D3D11VA"), "{file}");
        assert!(
            frames
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|frame| !frame.rgb.is_empty())
        );
        println!("{file}: GPU D3D11VA produziu RGB");
        drop(player);
    }
}

#[test]
fn cancelling_rtsp_open_interrupts_blocking_io() {
    use std::{io::Read, net::TcpListener, sync::mpsc};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let mut bytes = [0; 4096];
        assert!(socket.read(&mut bytes).unwrap() > 0);
        tx.send(()).unwrap();
        assert_eq!(socket.read(&mut bytes).unwrap(), 0);
    });
    let uri = sim_media::dahua_rtsp_url(endpoint, "fixture", "synthetic", 0).unwrap();
    let player = Player::start(uri, Default::default(), 1);
    rx.recv_timeout(Duration::from_secs(4)).unwrap();
    let started = Instant::now();
    drop(player);
    assert!(started.elapsed() < Duration::from_secs(3));
    server.join().unwrap();
}
