use super::*;
use sim_media::{Player, StreamQuality};
use tokio::time::Instant;
#[cfg(test)]
mod tests;

struct Running<'a> {
    request: VideoRequest,
    player: Option<Player>,
    retry_at: Option<Instant>,
    failures: u32,
    quality: StreamQuality,
    fallback: bool,
    #[cfg(target_os = "linux")]
    tunnel: Option<sim_p2p_t2u::Tunnel<'a>>,
    #[cfg(not(target_os = "linux"))]
    _lifetime: std::marker::PhantomData<&'a ()>,
}
impl Running<'_> {
    fn new(request: VideoRequest) -> Self {
        Self {
            request,
            player: None,
            retry_at: Some(Instant::now()),
            failures: 0,
            quality: request.quality,
            fallback: false,
            #[cfg(target_os = "linux")]
            tunnel: None,
            #[cfg(not(target_os = "linux"))]
            _lifetime: std::marker::PhantomData,
        }
    }
}
pub(super) async fn run(
    ctx: &Context<'_>,
    control: std::net::SocketAddr,
    available: &[u16],
    requests: &mut watch::Receiver<Requests>,
) -> Result<(), Failure> {
    let mut failures = 0;
    loop {
        while requests.borrow().iter().all(Option::is_none) {
            if requests.changed().await.is_err() {
                return Ok(());
            }
        }
        let result = group(ctx, control, available, requests).await;
        if let Err(error) = result {
            let retry = transient(error.as_ref());
            for (slot, r) in requests.borrow().iter().enumerate() {
                if let Some(r) = r {
                    ctx.clear(slot);
                    ctx.video(
                        slot,
                        *r,
                        if retry {
                            VideoState::Reconnecting
                        } else {
                            VideoState::Error
                        },
                        format!("Túnel de vídeo: {error}"),
                    );
                }
            }
            if retry {
                failures += 1;
                tokio::select! {_=requests.changed()=>{},_=tokio::time::sleep(retry_delay(failures))=>{}}
            } else if requests.changed().await.is_err() {
                return Ok(());
            }
        } else {
            failures = 0;
        }
    }
}
async fn group(
    ctx: &Context<'_>,
    control: std::net::SocketAddr,
    available: &[u16],
    requests: &mut watch::Receiver<Requests>,
) -> Result<(), Failure> {
    if ctx.device.config.manufacturer != Manufacturer::Dahua {
        return Err("Vídeo disponível para Intelbras/Dahua.".into());
    }
    // Windows compartilha um túnel RTSP por DVR; Linux prepara cada player em drive.
    let tunnel = match ctx.relay {
        #[cfg(windows)]
        Some(relay) => {
            let opening = relay.tunnel(&ctx.device.config.address, 554);
            tokio::pin!(opening);
            loop {
                tokio::select! {
                    result=&mut opening=>break Some(result?),
                    changed=requests.changed()=>{if changed.is_err() || requests.borrow().iter().all(Option::is_none){return Ok(());}}
                }
            }
        }
        // Cada player Linux aguarda um canal nativo independente antes de
        // iniciar o prazo de leitura do FFmpeg (ver drive).
        #[cfg(target_os = "linux")]
        Some(_) => None::<sim_p2p_t2u::Tunnel<'_>>,
        None => None,
    };
    let endpoint = tunnel
        .as_ref()
        .map(|t| t.endpoint())
        .unwrap_or_else(|| std::net::SocketAddr::new(control.ip(), 554));
    // Declarados após o túnel: Drop sempre fecha decoders antes de liberar a porta.
    let playback = drive(ctx, endpoint, available, requests);
    tokio::pin!(playback);
    if let Some(tunnel) = tunnel.as_ref() {
        let mut health = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                result=&mut playback=>return result,
                _=health.tick()=>{let state=tunnel.status()?;if state!=1{return Err(sim_p2p_t2u::Error::Status(state).into());}}
            }
        }
    } else {
        playback.await
    }
}
#[cfg_attr(windows, allow(unused_labels))]
async fn drive(
    ctx: &Context<'_>,
    endpoint: std::net::SocketAddr,
    available: &[u16],
    requests: &mut watch::Receiver<Requests>,
) -> Result<(), Failure> {
    let mut active: [Option<Running>; CAPACITY] = std::array::from_fn(|_| None);
    'drive: loop {
        let desired = *requests.borrow_and_update();
        for slot in 0..CAPACITY {
            if active[slot].as_ref().map(|r| r.request) != desired[slot] {
                active[slot] = None;
                ctx.clear(slot);
                if let Some(request) = desired[slot] {
                    let mut running = Running::new(request);
                    if !available.contains(&request.channel) {
                        running.retry_at = None;
                        ctx.video(
                            slot,
                            request,
                            VideoState::Error,
                            "Canal não encontrado no DVR.".into(),
                        );
                    }
                    active[slot] = Some(running);
                }
            }
            let Some(running) = active[slot].as_mut() else {
                continue;
            };
            if running.player.is_none() && running.retry_at.is_some_and(|at| at <= Instant::now()) {
                #[cfg(target_os = "linux")]
                let endpoint = if let Some(relay) = ctx.relay {
                    // O túnel compartilhado Windows já aceita conexões prontas.
                    // No Linux, preparar cada canal antes do decoder evita que
                    // uma negociação P2P de até 20s consuma seu prazo RTSP de 8s.
                    let result = prepare_tunnel(
                        relay,
                        &ctx.device.config.address,
                        slot,
                        running.request,
                        requests,
                    )
                    .await;
                    match result {
                        Ok(Some(tunnel)) => {
                            let endpoint = tunnel.endpoint();
                            running.tunnel = Some(tunnel);
                            endpoint
                        }
                        Ok(None) => continue 'drive,
                        Err(error) => {
                            running.failures += 1;
                            running.retry_at = Some(Instant::now() + retry_delay(running.failures));
                            ctx.video(
                                slot,
                                running.request,
                                VideoState::Reconnecting,
                                format!("Túnel de vídeo: {error}"),
                            );
                            continue;
                        }
                    }
                } else {
                    endpoint
                };
                let uri = sim_media::dahua_rtsp_url_quality(
                    endpoint,
                    &ctx.device.config.username,
                    ctx.password,
                    running.request.channel,
                    running.quality,
                )?;
                ctx.video(
                    slot,
                    running.request,
                    if running.failures == 0 {
                        VideoState::Connecting
                    } else {
                        VideoState::Reconnecting
                    },
                    format!(
                        "{} canal {} · {}…",
                        if running.failures == 0 {
                            "Conectando"
                        } else {
                            "Reconectando"
                        },
                        u32::from(running.request.channel) + 1,
                        running.quality.label()
                    ),
                );
                running.player = Some(Player::start_accelerated(
                    uri,
                    ctx.frames[slot].clone(),
                    running.request.generation,
                    running.request.gpu,
                ));
                running.retry_at = None;
                #[cfg(target_os = "linux")]
                if *requests.borrow() != desired {
                    continue 'drive;
                }
            }
            while let Some(event) = running.player.as_ref().and_then(Player::try_event) {
                match event {
                    sim_media::Event::Started {
                        codec,
                        width,
                        height,
                        backend,
                    } => {
                        running.failures = 0;
                        ctx.playing(
                            slot,
                            running.request,
                            format!(
                                "{} · {}×{} · {}{} · {}",
                                codec.to_uppercase(),
                                width,
                                height,
                                running.quality.label(),
                                if running.fallback {
                                    " (HD indisponível)"
                                } else {
                                    ""
                                },
                                backend
                            ),
                            format!("{width}×{height} · {}", running.quality.label()),
                        );
                    }
                    sim_media::Event::Ended(result) => {
                        running.player = None;
                        #[cfg(target_os = "linux")]
                        {
                            running.tunnel = None;
                        }
                        ctx.clear(slot);
                        let error = result.err().unwrap_or(sim_media::Error::Ended);
                        if running.quality == StreamQuality::Hd
                            && running.request.fallback
                            && error.substream_fallback()
                        {
                            running.quality = StreamQuality::Sd;
                            running.fallback = true;
                            running.retry_at = Some(Instant::now());
                            ctx.video(
                                slot,
                                running.request,
                                VideoState::Connecting,
                                "HD indisponível; abrindo SD.".into(),
                            );
                        } else if error.retryable() {
                            running.failures += 1;
                            running.retry_at = Some(Instant::now() + retry_delay(running.failures));
                            ctx.video(
                                slot,
                                running.request,
                                VideoState::Reconnecting,
                                format!("Sem sinal · reconectando: {error}"),
                            );
                        } else {
                            running.retry_at = None;
                            ctx.video(
                                slot,
                                running.request,
                                VideoState::Error,
                                format!("Vídeo indisponível: {error}"),
                            );
                        }
                        break;
                    }
                }
            }
        }
        if desired.iter().all(Option::is_none) {
            return Ok(());
        }
        tokio::select! {changed=requests.changed()=>{if changed.is_err(){return Ok(());}},_=tokio::time::sleep(Duration::from_millis(40))=>{}}
    }
}

#[cfg(target_os = "linux")]
async fn prepare_tunnel<'a>(
    relay: &'a Relay,
    serial: &str,
    slot: usize,
    request: VideoRequest,
    requests: &mut watch::Receiver<Requests>,
) -> Result<Option<sim_p2p_t2u::Tunnel<'a>>, sim_p2p_t2u::Error> {
    let opening = relay.tunnel(serial, 554);
    tokio::pin!(opening);
    loop {
        tokio::select! {
            result = &mut opening => return result.map(Some),
            changed = requests.changed() => {
                // Alterações de outra posição não cancelam esta negociação.
                if changed.is_err() || requests.borrow()[slot] != Some(request) {
                    return Ok(None);
                }
            }
        }
    }
}
