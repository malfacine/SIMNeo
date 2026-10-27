use super::*;
use sim_core::recording::Stamp;
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicU32,
    time::Duration,
};

pub fn playback_url(
    endpoint: SocketAddr,
    user: &str,
    password: &str,
    channel: u16,
    start: &Stamp,
    end: &Stamp,
) -> Result<Zeroizing<String>, Error> {
    let mut url =
        url::Url::parse(&format!("rtsp://{endpoint}/cam/playback")).map_err(|_| Error::Address)?;
    url.set_username(user).map_err(|_| Error::Address)?;
    url.set_password(Some(password))
        .map_err(|_| Error::Address)?;
    url.query_pairs_mut()
        .append_pair("channel", &(u32::from(channel) + 1).to_string())
        .append_pair("starttime", &start.rtsp())
        .append_pair("endtime", &end.rtsp());
    Ok(Zeroizing::new(url.into()))
}

pub fn export_url(
    endpoint: SocketAddr,
    user: &str,
    password: &str,
    channel: u16,
    start: &Stamp,
    end: &Stamp,
    quality: StreamQuality,
) -> Result<Zeroizing<String>, Error> {
    let uri = playback_url(endpoint, user, password, channel, start, end)?;
    if quality == StreamQuality::Hd {
        return Ok(uri);
    }
    let mut url = url::Url::parse(&uri).map_err(|_| Error::Address)?;
    url.query_pairs_mut().append_pair("subtype", "1");
    Ok(Zeroizing::new(url.into()))
}

#[derive(Default)]
pub struct PlaybackControl {
    pub paused: AtomicBool,
    pub position_ms: AtomicU64,
    pub buffering: AtomicBool,
    speed: AtomicU32,
}
pub(crate) struct PlaybackClock {
    wall: std::time::Instant,
    media_ms: f64,
}
impl Default for PlaybackClock {
    fn default() -> Self {
        Self {
            wall: std::time::Instant::now(),
            media_ms: 0.0,
        }
    }
}
impl PlaybackClock {
    pub(crate) fn exclude(&mut self, duration: Duration) {
        self.wall += duration;
    }
}
impl PlaybackControl {
    pub fn set_speed(&self, speed: u32) {
        if [1, 2, 4, 8, 16].contains(&speed) {
            self.speed.store(speed, Ordering::Release);
        }
    }
    pub fn speed(&self) -> u32 {
        self.speed.load(Ordering::Acquire).max(1)
    }
    pub(crate) fn pace(
        &self,
        millis: u64,
        clock: &mut PlaybackClock,
        cancelled: &AtomicBool,
    ) -> bool {
        loop {
            if cancelled.load(Ordering::Acquire) {
                return false;
            }
            if self.paused.load(Ordering::Acquire) {
                clock.wall = std::time::Instant::now();
                std::thread::sleep(Duration::from_millis(10));
                clock.wall = std::time::Instant::now();
                continue;
            }
            let now = std::time::Instant::now();
            let speed = f64::from(self.speed());
            clock.media_ms += now.duration_since(clock.wall).as_secs_f64() * 1000.0 * speed;
            clock.wall = now;
            if clock.media_ms >= millis as f64 {
                break;
            }
            let step = ((millis as f64 - clock.media_ms) / speed)
                .ceil()
                .clamp(1.0, 5.0) as u64;
            std::thread::sleep(Duration::from_millis(step));
        }
        self.position_ms.store(millis, Ordering::Release);
        true
    }
}

pub struct Export {
    cancelled: Arc<AtomicBool>,
    pub progress: Arc<AtomicU32>,
    worker: Option<JoinHandle<Result<(), String>>>,
}
impl Export {
    pub fn start(uri: Zeroizing<String>, destination: PathBuf, duration: u32) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let progress = Arc::new(AtomicU32::new(0));
        let percent = progress.clone();
        let worker =
            std::thread::spawn(move || export(&uri, &destination, duration, stop, &percent));
        Self {
            cancelled,
            progress,
            worker: Some(worker),
        }
    }
    pub fn finished(&self) -> bool {
        self.worker.as_ref().is_none_or(|w| w.is_finished())
    }
    pub fn result(&mut self) -> Result<(), String> {
        self.worker
            .take()
            .ok_or("Exportação já encerrada.")?
            .join()
            .map_err(|_| "A tarefa de exportação foi interrompida.")?
    }
}
impl Drop for Export {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct Partial(PathBuf);
impl Drop for Partial {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn export(
    uri: &str,
    destination: &Path,
    duration: u32,
    cancelled: Arc<AtomicBool>,
    progress: &AtomicU32,
) -> Result<(), String> {
    if duration == 0 || duration > 86400 {
        return Err("Intervalo inválido para exportação.".into());
    }
    if destination.exists() {
        return Err("O arquivo já existe. Escolha outro nome.".into());
    }
    super::initialize().map_err(|_| "Não foi possível iniciar o exportador.")?;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let partial = Partial(destination.with_extension(format!(
        "mp4.partial-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial.0)
        .map_err(|_| "Não foi possível criar o arquivo de destino.")?;
    let process = || -> Result<(), Error> {
        let mut input = super::open(uri, cancelled.clone())?;
        let stream = input
            .context
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(Error::Unsupported)?;
        if !matches!(
            stream.parameters().id(),
            ffmpeg::codec::Id::H264 | ffmpeg::codec::Id::HEVC
        ) {
            return Err(Error::Unsupported);
        }
        let index = stream.index();
        let time_base = stream.time_base();
        let mut output = ffmpeg::format::output_as(&partial.0, "mp4")?;
        {
            let mut track = output.add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))?;
            track.set_parameters(stream.parameters());
            // Parameters are owned by the new output stream; discard source container tags.
            unsafe {
                (*track.parameters().as_mut_ptr()).codec_tag = 0;
            }
        }
        let mut options = ffmpeg::Dictionary::new();
        options.set("movflags", "+faststart");
        output.write_header_with(options)?;
        let output_base = output.stream(0).ok_or(Error::Unsupported)?.time_base();
        let mut first = None;
        let mut next_timestamp = 0;
        let mut count = 0;
        let mut last_ms = 0;
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(Error::Ended);
            }
            input.interrupt.arm();
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut input.context) {
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => break,
                Err(ffmpeg::Error::Exit)
                    if last_ms + 2000 >= u64::from(duration) * 1000
                        && !cancelled.load(Ordering::Acquire) =>
                {
                    break;
                }
                Err(e) => return Err(e.into()),
            }
            if packet.stream() != index {
                continue;
            }
            if first.is_none() && !packet.is_key() {
                continue;
            }
            let timestamp = packet.dts().or(packet.pts()).unwrap_or(next_timestamp);
            next_timestamp = timestamp + packet.duration().max(1);
            if packet.dts().is_none() {
                packet.set_dts(Some(timestamp));
            }
            if packet.pts().is_none() {
                packet.set_pts(Some(timestamp));
            }
            let origin = *first.get_or_insert(timestamp);
            let millis =
                ((timestamp - origin).max(0) as f64 * f64::from(time_base) * 1000.0) as u64;
            if millis >= u64::from(duration) * 1000 {
                break;
            }
            if let Some(pts) = packet.pts() {
                packet.set_pts(Some(pts - origin));
            }
            if let Some(dts) = packet.dts() {
                packet.set_dts(Some(dts - origin));
            }
            packet.rescale_ts(time_base, output_base);
            packet.set_stream(0);
            packet.set_position(-1);
            packet.write_interleaved(&mut output)?;
            count += 1;
            last_ms = millis;
            progress.store(
                (millis * 100 / (u64::from(duration) * 1000)).min(99) as u32,
                Ordering::Release,
            );
        }
        if count == 0 || last_ms + 2000 < u64::from(duration) * 1000 {
            return Err(Error::Ended);
        }
        output.write_trailer()?;
        Ok(())
    };
    process().map_err(|e| format!("Exportação não concluída: {e}"))?;
    if cancelled.load(Ordering::Acquire) {
        return Err("Exportação cancelada.".into());
    }
    // Publish only a fully closed MP4, without replacing another file.
    publish(&partial.0, destination).map_err(
        |_| "Não foi possível concluir o arquivo. Confira a pasta e se o nome já existe.",
    )?;
    progress.store(100, Ordering::Release);
    Ok(())
}

#[cfg(windows)]
fn publish(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileW(existing: *const u16, new: *const u16) -> i32;
    }
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<_> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // MoveFileW works on USB/network volumes and refuses an existing destination.
    if unsafe { MoveFileW(source.as_ptr(), destination.as_ptr()) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
fn publish(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::hard_link(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speed_changes_pacing_without_changing_recorded_position() {
        let control = PlaybackControl::default();
        let cancelled = AtomicBool::new(false);
        let normal = std::time::Instant::now();
        assert!(control.pace(400, &mut PlaybackClock::default(), &cancelled));
        let normal = normal.elapsed();
        control.set_speed(16);
        let fast = std::time::Instant::now();
        assert!(control.pace(400, &mut PlaybackClock::default(), &cancelled));
        assert!(fast.elapsed() < normal / 3);
        assert_eq!(control.position_ms.load(Ordering::Acquire), 400);
        for speed in [1, 2, 4, 8, 16] {
            control.set_speed(speed);
            assert_eq!(control.speed(), speed);
        }
        control.set_speed(3);
        assert_eq!(control.speed(), 16);
    }
}
