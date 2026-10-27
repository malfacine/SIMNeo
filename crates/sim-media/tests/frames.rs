use sim_media::{dahua_rtsp_url, display_size, packed_rgb};

#[test]
fn packs_rows_without_native_padding() {
    let data = [1, 2, 3, 4, 5, 6, 99, 99, 7, 8, 9, 10, 11, 12, 99, 99];
    assert_eq!(
        packed_rgb(&data, 2, 2, 8).unwrap(),
        vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    );
}

#[test]
fn rejects_invalid_stride_and_truncated_frames() {
    assert!(packed_rgb(&[0; 10], 2, 2, 8).is_err());
    assert!(packed_rgb(&[0; 16], 2, 2, 4).is_err());
    assert!(packed_rgb(&[], 0, 2, 0).is_err());
    assert!(display_size(0, 10).is_err());
    assert!(display_size(u32::MAX, 10).is_err());
}

#[test]
fn scales_to_display_bounds_without_changing_aspect() {
    assert_eq!(display_size(1920, 1080).unwrap(), (1280, 720));
    assert_eq!(display_size(640, 480).unwrap(), (640, 480));
}

#[test]
fn rtsp_uses_one_based_channel_and_substream() {
    let uri = dahua_rtsp_url("127.0.0.1:12345".parse().unwrap(), "fixture", "p@ss", 0).unwrap();
    assert!(uri.ends_with("/cam/realmonitor?channel=1&subtype=1"));
    assert!(uri.contains("p%40ss@127.0.0.1:12345"));
}

#[test]
fn main_stream_is_explicit_and_authentication_is_never_retried() {
    use sim_media::{Error, StreamQuality, dahua_rtsp_url_quality};
    let uri = dahua_rtsp_url_quality(
        "127.0.0.1:12345".parse().unwrap(),
        "fixture",
        "synthetic",
        2,
        StreamQuality::Hd,
    )
    .unwrap();
    assert!(uri.ends_with("channel=3&subtype=0"));
    for code in [
        ffmpeg_next::Error::HttpUnauthorized,
        ffmpeg_next::Error::HttpForbidden,
        ffmpeg_next::Error::HttpNotFound,
    ] {
        assert!(!Error::Native(code.into()).retryable());
    }
    assert!(Error::Native(ffmpeg_next::Error::HttpNotFound.into()).substream_fallback());
    assert!(!Error::Native(ffmpeg_next::Error::HttpUnauthorized.into()).substream_fallback());
    assert!(Error::Ended.retryable());
    assert!(!Error::Frame.retryable());
}
