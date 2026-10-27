use super::*;
use sim_core::recording::{Recording, Stamp};
use sim_media::archive::{Export, PlaybackControl};
use std::sync::{Arc, atomic::Ordering};

pub const SLOTS: usize = 1;
pub type ArchiveFrames = [sim_media::FrameSlot; SLOTS];
pub enum Job {
    Search {
        channels: Vec<u16>,
        start: Stamp,
        end: Stamp,
    },
    Play {
        channels: Vec<u16>,
        start: Stamp,
        end: Stamp,
        frames: ArchiveFrames,
        controls: [Arc<PlaybackControl>; SLOTS],
        gpu: bool,
    },
    Export {
        channel: u16,
        start: Stamp,
        end: Stamp,
        path: PathBuf,
        quality: sim_media::StreamQuality,
    },
}
pub enum Update {
    SlotError { slot: usize, message: String },
    Found(Vec<Recording>),
    Playing,
    Finished,
    ExportProgress(u32),
    Exported(PathBuf),
    Error(String),
}
pub(super) async fn run(
    token: u64,
    device: Device,
    db: PathBuf,
    pool: Rc<RelayPool>,
    events: Sender<Event>,
    job: Job,
) -> Result<(), Failure> {
    if device.config.manufacturer != Manufacturer::Dahua {
        return Err("Reprodução disponível para Intelbras/Dahua nesta versão.".into());
    }
    let password = Store::open(&db)?.password(device.id)?;
    let ctx = Context {
        device: &device,
        password: &password,
        events: &events,
        frames: std::array::from_fn(|_| Default::default()),
        relay: None,
    };
    if device.config.connection == ConnectionMode::Address {
        let primary = async {
            let endpoint = resolve_address(&device).await?;
            let session = Session::open(&ctx, endpoint).await?;
            Ok::<_, Failure>((endpoint, session))
        }
        .await;
        match primary {
            Ok((endpoint, Session::Dahua(mut session))) => {
                return connected(token, &ctx, &mut session, endpoint, None, job).await;
            }
            Ok(_) => return Err("Protocolo de reprodução não suportado.".into()),
            Err(e) if device.config.serial.is_none() => return Err(e),
            Err(_) => {}
        }
    }
    let serial = device
        .config
        .serial
        .as_deref()
        .unwrap_or(&device.config.address);
    let relay = pool.get()?;
    let control = relay.tunnel(serial, device.config.port).await?;
    let Session::Dahua(mut session) = Session::open(&ctx, control.endpoint()).await? else {
        return Err("Protocolo de reprodução não suportado.".into());
    };
    connected(
        token,
        &ctx,
        &mut session,
        control.endpoint(),
        Some((&relay, serial)),
        job,
    )
    .await
}
async fn connected(
    token: u64,
    ctx: &Context<'_>,
    session: &mut sim_proto_dahua::Session,
    endpoint: std::net::SocketAddr,
    relay: Option<(&Relay, &str)>,
    job: Job,
) -> Result<(), Failure> {
    let publish = |update| {
        let _ = ctx.events.send(Event::Archive { token, update });
    };
    if let Job::Search {
        channels,
        start,
        end,
    } = &job
    {
        let mut found = Vec::new();
        for &channel in channels {
            found.extend(session.recordings(channel, start, end, 0).await?);
            session.keep_alive().await?;
        }
        found.sort_by(|a, b| (a.channel, &a.start).cmp(&(b.channel, &b.start)));
        publish(Update::Found(found));
        return Ok(());
    }
    let tunnel = match relay {
        Some((relay, serial)) => Some(relay.tunnel(serial, 554).await?),
        None => None,
    };
    let endpoint = tunnel
        .as_ref()
        .map(|t| t.endpoint())
        .unwrap_or_else(|| std::net::SocketAddr::new(endpoint.ip(), 554));
    let mut keep = tokio::time::interval(session.keep_alive_interval / 2);
    keep.tick().await;
    match job {
        Job::Play {
            channels,
            start,
            end,
            frames,
            controls,
            gpu,
        } => {
            let speed = controls[0].speed();
            let proxy = if speed > 1 {
                Some(super::rtsp_speed::Proxy::open(endpoint, speed).await?)
            } else {
                None
            };
            let endpoint = proxy.as_ref().map(|p| p.endpoint).unwrap_or(endpoint);
            let mut players: Vec<_> = channels
                .iter()
                .enumerate()
                .map(|(slot, &channel)| {
                    sim_media::archive::playback_url(
                        endpoint,
                        &ctx.device.config.username,
                        ctx.password,
                        channel,
                        &start,
                        &end,
                    )
                    .map(|uri| {
                        sim_media::Player::start_playback(
                            uri,
                            frames[slot].clone(),
                            token,
                            gpu,
                            controls[slot].clone(),
                        )
                    })
                })
                .collect::<Result<_, _>>()?;
            publish(Update::Playing);
            let mut done = vec![false; players.len()];
            loop {
                tokio::select! {_=keep.tick()=>session.keep_alive().await?,_=tokio::time::sleep(Duration::from_millis(50))=>{}}
                for (slot, player) in players.iter_mut().enumerate() {
                    while let Some(event) = player.try_event() {
                        if let sim_media::Event::Ended(result) = event {
                            match result {
                                Ok(()) | Err(sim_media::Error::Ended) => done[slot] = true,
                                Err(ref error)
                                    if error.interrupted()
                                        && controls[slot].position_ms.load(Ordering::Acquire)
                                            + 2000
                                            >= u64::from(end.second() - start.second()) * 1000 =>
                                {
                                    done[slot] = true
                                }
                                Err(e) => {
                                    done[slot] = true;
                                    publish(Update::SlotError {
                                        slot,
                                        message: format!(
                                            "Não foi possível reproduzir este canal: {e}"
                                        ),
                                    });
                                }
                            }
                        }
                    }
                }
                if done.iter().all(|n| *n) {
                    publish(Update::Finished);
                    return Ok(());
                }
            }
        }
        Job::Export {
            channel,
            start,
            end,
            path,
            quality,
        } => {
            let subtype = if quality == sim_media::StreamQuality::Hd {
                0
            } else {
                1
            };
            let records = session.recordings(channel, &start, &end, subtype).await?;
            if records.is_empty() {
                return Err(format!("Não há gravações {} neste canal e período. Essa qualidade precisa estar gravada no DVR.",quality.label()).into());
            }
            let uri = sim_media::archive::export_url(
                endpoint,
                &ctx.device.config.username,
                ctx.password,
                channel,
                &start,
                &end,
                quality,
            )?;
            let mut export = Export::start(uri, path.clone(), end.second() - start.second());
            let mut previous = 101;
            while !export.finished() {
                tokio::select! {_=keep.tick()=>session.keep_alive().await?,_=tokio::time::sleep(Duration::from_millis(100))=>{}}
                let percent = export.progress.load(Ordering::Acquire);
                if percent != previous {
                    publish(Update::ExportProgress(percent));
                    previous = percent;
                }
            }
            export.result()?;
            publish(Update::Exported(path));
            Ok(())
        }
        Job::Search { .. } => unreachable!(),
    }
}
