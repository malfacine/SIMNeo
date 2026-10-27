use sim_media::archive::{Export, PlaybackControl, playback_url};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/media")
        .join(name)
}
fn finish(export: &mut Export) {
    let until = Instant::now() + Duration::from_secs(5);
    while !export.finished() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn remuxes_h264_and_h265_without_changing_codec_or_original_resolution() {
    for name in ["test-h264.mp4", "test-h265.mp4", "test-h264-1080p.mp4"] {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("clip.mp4");
        let mut export = Export::start(
            fixture(name).to_string_lossy().into_owned().into(),
            path.clone(),
            1,
        );
        finish(&mut export);
        export.result().unwrap();
        let mut input = ffmpeg_next::format::input(&path).unwrap();
        let source = ffmpeg_next::format::input(&fixture(name)).unwrap();
        let dimensions = |context: &ffmpeg_next::format::context::Input| {
            let parameters = context
                .streams()
                .best(ffmpeg_next::media::Type::Video)
                .unwrap()
                .parameters();
            let decoder = ffmpeg_next::codec::context::Context::from_parameters(parameters)
                .unwrap()
                .decoder()
                .video()
                .unwrap();
            (decoder.width(), decoder.height())
        };
        assert_eq!(dimensions(&input), dimensions(&source));
        if name == "test-h264-1080p.mp4" {
            assert_eq!(dimensions(&input), (1920, 1080));
        }
        assert_eq!(
            input
                .streams()
                .best(ffmpeg_next::media::Type::Video)
                .unwrap()
                .parameters()
                .id(),
            source
                .streams()
                .best(ffmpeg_next::media::Type::Video)
                .unwrap()
                .parameters()
                .id()
        );
        assert!(input.packets().count() > 0);
        assert_eq!(export.progress.load(Ordering::Acquire), 100);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }
}
#[test]
fn selects_original_main_or_extra_recording_stream() {
    let endpoint = "127.0.0.1:554".parse().unwrap();
    let (start, end) =
        sim_core::recording::period("2026-10-26 13:39:36", "2026-10-26 13:44:36").unwrap();
    for (quality, subtype) in [
        (sim_media::StreamQuality::Hd, None),
        (sim_media::StreamQuality::Sd, Some("1")),
    ] {
        let uri = sim_media::archive::export_url(
            endpoint,
            "fixture",
            "synthetic",
            1,
            &start,
            &end,
            quality,
        )
        .unwrap();
        let parsed = url::Url::parse(&uri).unwrap();
        let query = parsed
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(query.get("subtype").map(|v| v.as_ref()), subtype);
        assert_eq!(query.get("channel").unwrap(), "2");
        assert_eq!(query.get("starttime").unwrap(), "2026_10_26_13_39_36");
    }
}
#[test]
fn rejects_overwrite_without_touching_existing_file() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("clip.mp4");
    std::fs::write(&path, b"preserve").unwrap();
    let mut export = Export::start(
        fixture("test-h264.mp4")
            .to_string_lossy()
            .into_owned()
            .into(),
        path.clone(),
        1,
    );
    finish(&mut export);
    assert!(export.result().is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"preserve");
}
#[test]
fn cancellation_interrupts_network_and_removes_partial_file() {
    use std::{io::Read, net::TcpListener, sync::mpsc};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let mut bytes = [0u8; 4096];
        assert!(socket.read(&mut bytes).unwrap() > 0);
        tx.send(()).unwrap();
        assert_eq!(socket.read(&mut bytes).unwrap(), 0);
    });
    let folder = tempfile::tempdir().unwrap();
    let destination = folder.path().join("clip.mp4");
    let (start, end) =
        sim_core::recording::period("2026-10-26 00:00:00", "2026-10-26 00:01:00").unwrap();
    let export = Export::start(
        playback_url(endpoint, "fixture", "synthetic", 0, &start, &end).unwrap(),
        destination.clone(),
        60,
    );
    rx.recv_timeout(Duration::from_secs(4)).unwrap();
    let started = Instant::now();
    drop(export);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    server.join().unwrap();
}
#[test]
fn playback_pause_holds_frames_and_resumes() {
    let control = Arc::new(PlaybackControl::default());
    control.paused.store(true, Ordering::Release);
    let frames: sim_media::FrameSlot = Default::default();
    let player = sim_media::Player::start_playback(
        fixture("test-h264.mp4")
            .to_string_lossy()
            .into_owned()
            .into(),
        frames.clone(),
        7,
        false,
        control.clone(),
    );
    std::thread::sleep(Duration::from_millis(150));
    assert!(frames.lock().unwrap().is_none());
    control.paused.store(false, Ordering::Release);
    let until = Instant::now() + Duration::from_secs(3);
    while frames.lock().unwrap().is_none() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(frames.lock().unwrap().as_ref().unwrap().generation, 7);
    drop(player);
}
