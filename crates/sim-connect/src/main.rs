use serde::Deserialize;
use sim_p2p_t2u::Client;
use sim_storage_import::read_protected_file;
use std::{error::Error, path::PathBuf, time::Duration};

#[derive(Deserialize)]
struct Coordinator {
    server: String,
    port: u16,
    key: String,
    tunnel_password: String,
    remote_ip: String,
}
impl Drop for Coordinator {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.key.zeroize();
        self.tunnel_password.zeroize();
    }
}
#[derive(Deserialize)]
struct Profile {
    #[serde(default = "default_name")]
    name: String,
    serial: String,
    username: String,
    password: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    dhip: bool,
}
impl Drop for Profile {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
    }
}
fn default_port() -> u16 {
    37777
}
fn default_name() -> String {
    "DVR importado".into()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !(3..=5).contains(&args.len()) {
        return Err("Uso: sim-connect <caminho-absoluto-libt2u.dll> <coordenador.dpapi> <dispositivo.dpapi> [segundos: 1–120]".into());
    }
    let archive_mode = args.get(4).is_some_and(|s| s == "--archive-probe");
    let video_mode = args.get(4).is_some_and(|s| s == "--video");
    if args.len() == 5 && !video_mode && !archive_mode {
        return Err("Opção desconhecida. Use --video para testar RTSP.".into());
    }
    if args[0] == "register" {
        let profile: Profile =
            serde_json::from_str(&read_protected_file(&PathBuf::from(&args[2]))?)
                .map_err(|_| "Perfil protegido inválido.")?;
        let db = PathBuf::from(&args[1]);
        if let Some(parent) = db.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut store = sim_storage_import::Store::open(&db)?;
        if store.list()?.iter().any(|d| {
            d.config.connection == sim_core::ConnectionMode::P2p
                && d.config.address == profile.serial
                && d.config.port == profile.port
                && d.config.username == profile.username
        }) {
            println!("Cadastro P2P já existente; preservado.");
            return Ok(());
        }
        store.insert_batch(&[sim_storage_import::NewDevice::new(
            sim_core::DeviceDraft {
                serial: None,
                connection: sim_core::ConnectionMode::P2p,
                name: profile.name.clone(),
                address: profile.serial.clone(),
                port: profile.port,
                username: profile.username.clone(),
                manufacturer: sim_core::Manufacturer::Dahua,
                channels: vec![],
            },
            profile.password.clone(),
        )])?;
        println!("Cadastro P2P criado com credencial protegida.");
        return Ok(());
    }
    let seconds = args
        .get(3)
        .map(|s| s.parse::<u64>())
        .transpose()?
        .unwrap_or(25);
    if !(1..=120).contains(&seconds) {
        return Err("Duração do teste deve estar entre 1 e 120 segundos.".into());
    }
    let coordinator: Coordinator =
        serde_json::from_str(&read_protected_file(&PathBuf::from(&args[1]))?)
            .map_err(|_| "Configuração do coordenador inválida.")?;
    let profile: Profile = serde_json::from_str(&read_protected_file(&PathBuf::from(&args[2]))?)
        .map_err(|_| "Configuração do dispositivo inválida.")?;
    let client = Client::load(
        &PathBuf::from(&args[0]),
        &coordinator.server,
        coordinator.port,
        &coordinator.key,
    )?;
    println!("SDK P2P validado e inicializado.");
    let tunnel = client
        .tunnel(
            &profile.serial,
            &coordinator.tunnel_password,
            &coordinator.remote_ip,
            profile.port,
            Duration::from_secs(20),
        )
        .await?;
    println!("Túnel P2P conectado.");
    let mut session = sim_proto_dahua::Session::connect(
        tunnel.endpoint(),
        &profile.username,
        &profile.password,
        if profile.dhip {
            sim_proto_dahua::WireMode::Dhip
        } else {
            sim_proto_dahua::WireMode::Legacy
        },
    )
    .await?;
    println!("Login do DVR autenticado em Rust.");
    let channels = session.channels().await?;
    println!("Canais consultados: {}.", channels.len());
    session.keep_alive().await?;
    println!("Keepalive confirmado.");
    if archive_mode {
        let (start, end) =
            sim_core::recording::period("2026-10-26 00:00:00", "2026-10-26 23:59:59")?;
        let records = session.recordings(1, &start, &end, 0).await?;
        println!("SIMNeo: gravações encontradas: {}", records.len());
        for record in records.iter().take(3) {
            println!(
                "SIMNeo: período {} → {} ({})",
                record.start.0, record.end.0, record.kind
            );
        }
        if let Some(record) = records.first() {
            let rtsp = client
                .tunnel(
                    &profile.serial,
                    &coordinator.tunnel_password,
                    &coordinator.remote_ip,
                    554,
                    Duration::from_secs(20),
                )
                .await?;
            let end = record.start.at_second(record.start.second() + 6);
            let uri = sim_media::archive::playback_url(
                rtsp.endpoint(),
                &profile.username,
                &profile.password,
                record.channel,
                &record.start,
                &end,
            )?;
            let target = PathBuf::from("data/archive-native-test.mp4");
            let mut export = sim_media::archive::Export::start(uri, target, 6);
            let until = tokio::time::Instant::now() + Duration::from_secs(45);
            while !export.finished() {
                if tokio::time::Instant::now() > until {
                    return Err("Prazo do teste de exportação excedido.".into());
                }
                session.keep_alive().await?;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            export.result()?;
            println!("SIMNeo: exportação MP4 de seis segundos concluída.");
        }
        session.logout().await?;
        return Ok(());
    }

    let rtsp_tunnel = if video_mode {
        Some(
            client
                .tunnel(
                    &profile.serial,
                    &coordinator.tunnel_password,
                    &coordinator.remote_ip,
                    554,
                    Duration::from_secs(20),
                )
                .await?,
        )
    } else {
        None
    };
    let frames: sim_media::FrameSlot = Default::default();
    let player = if let Some(tunnel) = &rtsp_tunnel {
        println!("Túnel RTSP conectado.");
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut probe = tokio::net::TcpStream::connect(tunnel.endpoint()).await?;
        probe
            .write_all(
                b"OPTIONS rtsp://127.0.0.1/ RTSP/1.0\r\nCSeq: 1\r\nUser-Agent: SimNEO\r\n\r\n",
            )
            .await?;
        let mut reply = [0u8; 4096];
        match tokio::time::timeout(Duration::from_secs(5), probe.read(&mut reply)).await {
            Ok(Ok(len)) => {
                let status = String::from_utf8_lossy(&reply[..len]);
                println!(
                    "RTSP OPTIONS: {}",
                    status.lines().next().unwrap_or("Resposta vazia")
                );
            }
            _ => println!("RTSP OPTIONS: sem resposta no prazo."),
        }
        drop(probe);
        Some(sim_media::Player::start(
            sim_media::dahua_rtsp_url(tunnel.endpoint(), &profile.username, &profile.password, 0)?,
            frames.clone(),
            1,
        ))
    } else {
        None
    };
    let mut decoded_frames = 0u64;
    let until = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let mut next_keepalive = tokio::time::Instant::now() + session.keep_alive_interval / 2;
    while tokio::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(30)).await;
        if let Some(player) = &player {
            while let Some(event) = player.try_event() {
                match event {
                    sim_media::Event::Started {
                        codec,
                        width,
                        height,
                        ..
                    } => println!("Vídeo decodificado: {codec} {width}x{height}."),
                    sim_media::Event::Ended(result) => {
                        result?;
                    }
                }
            }
        }
        if frames.lock().ok().and_then(|mut f| f.take()).is_some() {
            decoded_frames += 1;
        }
        if tokio::time::Instant::now() >= next_keepalive {
            session.keep_alive().await?;
            println!("Keepalive periódico confirmado.");
            next_keepalive = tokio::time::Instant::now() + session.keep_alive_interval / 2;
        }
    }
    drop(player);
    drop(rtsp_tunnel);
    if video_mode {
        println!("Frames recebidos pelo consumidor: {decoded_frames}.");
        if decoded_frames == 0 {
            return Err("Nenhum frame foi decodificado no prazo do teste.".into());
        }
    }
    session.logout().await?;
    drop(tunnel);
    drop(client);
    println!("Sessão encerrada por TCP; túnel liberado.");
    Ok(())
}
