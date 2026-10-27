//! Vídeo e exportação em processo, sem executáveis FFmpeg auxiliares.

use ffmpeg_next as ffmpeg;
use std::{
    ffi::c_void,
    net::SocketAddr,
    ptr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use thiserror::Error;
use zeroize::Zeroizing;
pub mod archive;
mod hardware;
mod playback_buffer;

#[derive(Debug, Error, Clone)]
pub enum Error {
    #[error("Frame de vídeo inválido ou resolução não suportada.")]
    Frame,
    #[error("Endereço ou credencial RTSP inválida.")]
    Address,
    #[error("Falha no pipeline de vídeo (código {0}).")]
    Native(i32),
    #[error("O stream não contém vídeo H.264/H.265 compatível.")]
    Unsupported,
    #[error("O stream de vídeo foi encerrado.")]
    Ended,
    #[error("Falha na decodificação por GPU.")]
    Hardware,
}
impl From<ffmpeg::Error> for Error {
    fn from(error: ffmpeg::Error) -> Self {
        Self::Native(error.into())
    }
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    pub generation: u64,
}
pub type FrameSlot = Arc<Mutex<Option<Frame>>>;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamQuality {
    #[default]
    Sd,
    Hd,
}
impl StreamQuality {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sd => "SD",
            Self::Hd => "HD",
        }
    }
}
impl Error {
    pub fn interrupted(&self) -> bool {
        matches!(self,Self::Native(code) if ffmpeg::Error::from(*code)==ffmpeg::Error::Exit)
    }
    pub fn retryable(&self) -> bool {
        match self {
            Self::Ended => true,
            Self::Native(code) => {
                !matches!(
                    ffmpeg::Error::from(*code),
                    ffmpeg::Error::HttpUnauthorized
                        | ffmpeg::Error::HttpForbidden
                        | ffmpeg::Error::HttpNotFound
                        | ffmpeg::Error::HttpBadRequest
                        | ffmpeg::Error::DecoderNotFound
                        | ffmpeg::Error::DemuxerNotFound
                        | ffmpeg::Error::ProtocolNotFound
                        | ffmpeg::Error::InvalidData
                ) && ![-12, -13, -22].contains(code)
            }
            _ => false,
        }
    }
    pub fn substream_fallback(&self) -> bool {
        matches!(self, Self::Unsupported | Self::Frame)
            || matches!(self, Self::Native(code) if ffmpeg::Error::from(*code) == ffmpeg::Error::HttpNotFound)
    }
}
pub enum Event {
    Started {
        codec: String,
        width: u32,
        height: u32,
        backend: &'static str,
    },
    Ended(Result<(), Error>),
}

pub fn dahua_rtsp_url(
    endpoint: SocketAddr,
    user: &str,
    password: &str,
    channel: u16,
) -> Result<Zeroizing<String>, Error> {
    dahua_rtsp_url_quality(endpoint, user, password, channel, StreamQuality::Sd)
}

pub fn dahua_rtsp_url_quality(
    endpoint: SocketAddr,
    user: &str,
    password: &str,
    channel: u16,
    quality: StreamQuality,
) -> Result<Zeroizing<String>, Error> {
    let mut url = url::Url::parse(&format!("rtsp://{endpoint}/cam/realmonitor"))
        .map_err(|_| Error::Address)?;
    url.set_username(user).map_err(|_| Error::Address)?;
    url.set_password(Some(password))
        .map_err(|_| Error::Address)?;
    url.set_query(Some(&format!(
        "channel={}&subtype={}",
        u32::from(channel) + 1,
        if quality == StreamQuality::Hd { 0 } else { 1 }
    )));
    Ok(Zeroizing::new(url.to_string()))
}

pub fn display_size(width: u32, height: u32) -> Result<(u32, u32), Error> {
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(Error::Frame);
    }
    let ratio = (1280.0 / f64::from(width))
        .min(720.0 / f64::from(height))
        .min(1.0);
    Ok((
        (f64::from(width) * ratio).round().max(1.0) as u32,
        (f64::from(height) * ratio).round().max(1.0) as u32,
    ))
}

pub fn packed_rgb(data: &[u8], width: u32, height: u32, stride: usize) -> Result<Vec<u8>, Error> {
    if width == 0 || height == 0 || width > 1280 || height > 720 {
        return Err(Error::Frame);
    }
    let row = width as usize * 3;
    let required = (height as usize - 1)
        .checked_mul(stride)
        .and_then(|n| n.checked_add(row))
        .ok_or(Error::Frame)?;
    if stride < row || data.len() < required {
        return Err(Error::Frame);
    }
    let mut rgb = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        rgb.extend_from_slice(&data[y * stride..y * stride + row]);
    }
    Ok(rgb)
}

pub struct Player {
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    events: std::sync::mpsc::Receiver<Event>,
}
impl Player {
    pub fn start(uri: Zeroizing<String>, frames: FrameSlot, generation: u64) -> Self {
        Self::start_accelerated(uri, frames, generation, false)
    }
    pub fn start_accelerated(
        uri: Zeroizing<String>,
        frames: FrameSlot,
        generation: u64,
        gpu: bool,
    ) -> Self {
        Self::start_controlled(uri, frames, generation, gpu, None)
    }
    pub fn start_playback(
        uri: Zeroizing<String>,
        frames: FrameSlot,
        generation: u64,
        gpu: bool,
        control: Arc<archive::PlaybackControl>,
    ) -> Self {
        Self::start_controlled(uri, frames, generation, gpu, Some(control))
    }
    fn start_controlled(
        uri: Zeroizing<String>,
        frames: FrameSlot,
        generation: u64,
        gpu: bool,
        control: Option<Arc<archive::PlaybackControl>>,
    ) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let (tx, events) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut result = decode(
                &uri,
                &stop,
                frames.clone(),
                generation,
                &tx,
                DecodeOptions {
                    gpu,
                    fallback: false,
                    control: control.as_deref(),
                },
            );
            if gpu && matches!(result, Err(Error::Hardware)) && !stop.load(Ordering::Acquire) {
                if let Ok(mut latest) = frames.lock() {
                    *latest = None;
                }
                result = decode(
                    &uri,
                    &stop,
                    frames,
                    generation,
                    &tx,
                    DecodeOptions {
                        gpu: false,
                        fallback: true,
                        control: control.as_deref(),
                    },
                );
            }
            let result = if stop.load(Ordering::Acquire) {
                Ok(())
            } else {
                result
            };
            let _ = tx.send(Event::Ended(result));
        });
        Self {
            cancelled,
            worker: Some(worker),
            events,
        }
    }
    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Interrupt {
    start: Instant,
    deadline_ms: AtomicU64,
    cancelled: Arc<AtomicBool>,
    read_stop: Arc<AtomicBool>,
}
impl Interrupt {
    fn arm(&self) {
        self.deadline_ms.store(
            self.start.elapsed().as_millis() as u64 + 8000,
            Ordering::Release,
        );
    }
}
unsafe extern "C" fn interrupted(opaque: *mut c_void) -> i32 {
    // SAFETY: o Box Interrupt vive até depois de fechar o contexto de entrada.
    let state = unsafe { &*opaque.cast::<Interrupt>() };
    i32::from(
        state.cancelled.load(Ordering::Acquire)
            || state.read_stop.load(Ordering::Acquire)
            || state.start.elapsed().as_millis() as u64
                >= state.deadline_ms.load(Ordering::Acquire),
    )
}
struct Input {
    context: ffmpeg::format::context::Input,
    interrupt: Box<Interrupt>,
}

fn open(uri: &str, cancelled: Arc<AtomicBool>) -> Result<Input, Error> {
    let mut interrupt = Box::new(Interrupt {
        start: Instant::now(),
        deadline_ms: AtomicU64::new(8000),
        cancelled,
        read_stop: Default::default(),
    });
    let mut path = Zeroizing::new(uri.as_bytes().to_vec());
    if path.contains(&0) {
        return Err(Error::Address);
    }
    path.push(0);
    let mut options = ffmpeg::Dictionary::new();
    options.set("rtsp_transport", "tcp");
    options.set("allowed_media_types", "video");
    options.set("rw_timeout", "8000000");
    options.set("analyzeduration", "2000000");
    options.set("probesize", "1000000");
    // SAFETY: AVFormatContext e Dictionary pertencem a esta função. Em erro,
    // fecha o contexto antes de destruir o callback; em sucesso, Input os retém
    // na ordem correta de destruição. URI/credenciais nunca são registradas.
    unsafe {
        let mut context = ffmpeg::ffi::avformat_alloc_context();
        if context.is_null() {
            return Err(Error::Native(-12));
        }
        (*context).interrupt_callback = ffmpeg::ffi::AVIOInterruptCB {
            callback: Some(interrupted),
            opaque: (&mut *interrupt as *mut Interrupt).cast(),
        };
        let mut raw_options = options.disown();
        let code = ffmpeg::ffi::avformat_open_input(
            &mut context,
            path.as_ptr().cast(),
            ptr::null(),
            &mut raw_options,
        );
        drop(ffmpeg::Dictionary::own(raw_options));
        if code < 0 {
            if !context.is_null() {
                ffmpeg::ffi::avformat_close_input(&mut context);
            }
            return Err(Error::Native(code));
        }
        interrupt.arm();
        let code = ffmpeg::ffi::avformat_find_stream_info(context, ptr::null_mut());
        if code < 0 {
            ffmpeg::ffi::avformat_close_input(&mut context);
            return Err(Error::Native(code));
        }
        Ok(Input {
            context: ffmpeg::format::context::Input::wrap(context),
            interrupt,
        })
    }
}

struct DecodeOptions<'a> {
    gpu: bool,
    fallback: bool,
    control: Option<&'a archive::PlaybackControl>,
}
fn initialize() -> Result<(), Error> {
    static INIT: OnceLock<Result<(), Error>> = OnceLock::new();
    INIT.get_or_init(|| {
        ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Quiet);
        ffmpeg::init().map_err(Error::from)
    })
    .clone()
}
fn decode(
    uri: &str,
    cancelled: &Arc<AtomicBool>,
    frames: FrameSlot,
    generation: u64,
    events: &std::sync::mpsc::Sender<Event>,
    options: DecodeOptions,
) -> Result<(), Error> {
    let DecodeOptions {
        gpu,
        fallback,
        control,
    } = options;
    initialize()?;
    let input = open(uri, cancelled.clone())?;
    let stream = input
        .context
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or(Error::Unsupported)?;
    let index = stream.index();
    let time_base = stream.time_base();
    let mut first_pts = None;
    let mut playback_clock = archive::PlaybackClock::default();
    let mut last_preview = None::<Instant>;
    let mut context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
    if !matches!(
        context.id(),
        ffmpeg::codec::Id::H264 | ffmpeg::codec::Id::HEVC
    ) {
        return Err(Error::Unsupported);
    }
    context.set_threading(ffmpeg::codec::threading::Config::count(2));
    let hardware = gpu && hardware::attach(&mut context);
    let mut decoder = context
        .decoder()
        .video()
        .map_err(|e| if hardware { Error::Hardware } else { e.into() })?;
    let codec = decoder.id().name().to_owned();
    let mut scaling = None;
    let mut announced = false;
    enum Source {
        Direct(Input),
        Buffered(playback_buffer::Reader),
    }
    let mut source = if let Some(control) = control {
        control.buffering.store(true, Ordering::Release);
        Source::Buffered(playback_buffer::Reader::start(
            input,
            index,
            f64::from(time_base),
            control.speed(),
            cancelled.clone(),
        ))
    } else {
        Source::Direct(input)
    };
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut packet = ffmpeg::Packet::empty();
        let result = match &mut source {
            Source::Direct(input) => {
                input.interrupt.arm();
                packet.read(&mut input.context).map_err(Error::from)
            }
            Source::Buffered(reader) => match reader.next(control.unwrap(), &mut playback_clock)? {
                Some(next) => {
                    packet = next;
                    Ok(())
                }
                None => Err(Error::from(ffmpeg::Error::Eof)),
            },
        };
        let eof = match result {
            Ok(()) => false,
            Err(_) if cancelled.load(Ordering::Acquire) => return Ok(()),
            Err(Error::Native(code)) if ffmpeg::Error::from(code) == ffmpeg::Error::Eof => true,
            Err(error) => return Err(error),
        };
        if !eof && packet.stream() != index {
            continue;
        }
        (if eof {
            decoder.send_eof()
        } else {
            decoder.send_packet(&packet)
        })
        .map_err(|e| if hardware { Error::Hardware } else { e.into() })?;
        let mut decoded = ffmpeg::util::frame::Video::empty();
        loop {
            match decoder.receive_frame(&mut decoded) {
                Ok(()) => {}
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
                Err(ffmpeg::Error::Eof) => break,
                Err(error) => {
                    return Err(if hardware {
                        Error::Hardware
                    } else {
                        error.into()
                    });
                }
            }
            let accelerated = decoded.format() == ffmpeg::format::Pixel::D3D11;
            if let Some(control) = control {
                let pts = decoded.timestamp().or(decoded.pts()).unwrap_or(0);
                let origin = *first_pts.get_or_insert(pts);
                let millis = ((pts - origin).max(0) as f64 * f64::from(time_base) * 1000.0) as u64;
                if !control.pace(millis, &mut playback_clock, cancelled) {
                    return Ok(());
                }
                // Acelerar não exige converter/copiar todos os quadros para a interface.
                if announced
                    && control.speed() > 1
                    && last_preview.is_some_and(|time| time.elapsed() < Duration::from_millis(40))
                {
                    continue;
                }
                last_preview = Some(Instant::now());
            }
            let mut downloaded = ffmpeg::util::frame::Video::empty();
            let decoded = if accelerated {
                // Destino vazio: o FFmpeg aloca e libera os buffers de transferência.
                let code = unsafe {
                    ffmpeg::ffi::av_hwframe_transfer_data(
                        downloaded.as_mut_ptr(),
                        decoded.as_ptr(),
                        0,
                    )
                };
                if code < 0 {
                    return Err(Error::Hardware);
                }
                &downloaded
            } else {
                &decoded
            };
            let (width, height) = display_size(decoded.width(), decoded.height())?;
            let spec = (decoded.format(), decoded.width(), decoded.height());
            if scaling.as_ref().is_none_or(|(old, _)| *old != spec) {
                scaling = Some((
                    spec,
                    ffmpeg::software::scaling::Context::get(
                        decoded.format(),
                        decoded.width(),
                        decoded.height(),
                        ffmpeg::format::Pixel::RGB24,
                        width,
                        height,
                        ffmpeg::software::scaling::flag::Flags::BILINEAR,
                    )?,
                ));
            }
            let mut rgb = ffmpeg::util::frame::Video::empty();
            scaling.as_mut().unwrap().1.run(decoded, &mut rgb)?;
            let packed = packed_rgb(rgb.data(0), width, height, rgb.stride(0))?;
            if let Ok(mut latest) = frames.lock() {
                *latest = Some(Frame {
                    width,
                    height,
                    rgb: packed,
                    generation,
                });
            }
            if !announced {
                let _ = events.send(Event::Started {
                    codec: codec.clone(),
                    width,
                    height,
                    backend: if accelerated {
                        "GPU D3D11VA"
                    } else if gpu || fallback {
                        "CPU (GPU indisponível)"
                    } else {
                        "CPU"
                    },
                });
                announced = true;
            }
        }
        if eof {
            return Err(Error::Ended);
        }
    }
}
