//! Mantém controle e quatro decoders para testar o transporte sem a interface.
#[cfg(target_os = "linux")]
include!(concat!(env!("OUT_DIR"), "/p2p_service.rs"));
#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::{
        io::Read,
        time::{Duration, Instant},
    };
    use zeroize::{Zeroize, Zeroizing};
    #[derive(serde::Deserialize)]
    struct Service {
        server: String,
        port: u16,
        key: String,
        tunnel_password: String,
        remote_ip: String,
    }
    #[derive(serde::Deserialize)]
    struct Profile {
        serial: String,
        username: String,
        password: String,
    }
    impl Drop for Profile {
        fn drop(&mut self) {
            self.serial.zeroize();
            self.password.zeroize();
            self.username.zeroize();
        }
    }
    impl Drop for Service {
        fn drop(&mut self) {
            self.key.zeroize();
            self.tunnel_password.zeroize();
        }
    }
    let mut input = Zeroizing::new(String::new());
    std::io::stdin().read_to_string(&mut input)?;
    let profile: Profile = serde_json::from_str(&input).map_err(|_| "Perfil inválido")?;
    let service: Service = serde_json::from_str(EMBEDDED_P2P_SERVICE.ok_or("Serviço ausente")?)
        .map_err(|_| "Serviço inválido")?;
    let library = std::path::PathBuf::from(std::env::args_os().nth(1).ok_or("Biblioteca ausente")?);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let client =
                sim_p2p_t2u::Client::load(&library, &service.server, service.port, &service.key)?;
            let control = client
                .tunnel(
                    &profile.serial,
                    &service.tunnel_password,
                    &service.remote_ip,
                    37777,
                    Duration::from_secs(20),
                )
                .await?;
            let mut session = sim_proto_dahua::Session::connect(
                control.endpoint(),
                &profile.username,
                &profile.password,
                sim_proto_dahua::WireMode::Legacy,
            )
            .await?;
            println!(
                "Controle autenticado; {} canais.",
                session.channels().await?.len()
            );
            let stop = Arc::new(AtomicBool::new(false));
            let task_stop = stop.clone();
            let keep = tokio::spawn(async move {
                while !task_stop.load(Ordering::Acquire) {
                    session.keep_alive().await?;
                    println!("Controle: keepalive confirmado.");
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
                Ok::<_, sim_proto_dahua::Error>(())
            });
            let mut channels = Vec::new();
            let frames: [sim_media::FrameSlot; 4] = std::array::from_fn(|_| Default::default());
            for number in 0..4 {
                let tunnel = client
                    .tunnel(
                        &profile.serial,
                        &service.tunnel_password,
                        &service.remote_ip,
                        554,
                        Duration::from_secs(20),
                    )
                    .await?;
                let uri = sim_media::dahua_rtsp_url(
                    tunnel.endpoint(),
                    &profile.username,
                    &profile.password,
                    number as u16,
                )?;
                let player = sim_media::Player::start(uri, frames[number].clone(), 1);
                channels.push(Some((player, tunnel)));
            }
            let started = Instant::now();
            let mut closed = false;
            let mut hd = false;
            let mut counts = [0u32; 4];
            let mut printed = 0;

            while started.elapsed() < Duration::from_secs(75) {
                if keep.is_finished() {
                    keep.await??;
                    return Err("Controle encerrado cedo".into());
                }
                for number in 0..4 {
                    if frames[number].lock().unwrap().take().is_some() {
                        counts[number] += 1;
                    }
                    if let Some((player, _)) = &channels[number] {
                        while let Some(event) = player.try_event() {
                            if let sim_media::Event::Ended(result) = event {
                                return Err(format!(
                                    "Decoder {} encerrado: {:?}",
                                    number + 1,
                                    result
                                )
                                .into());
                            }
                        }
                    }
                }
                if started.elapsed().as_secs() / 5 > printed {
                    printed = started.elapsed().as_secs() / 5;
                    println!("{}s: frames {:?}", started.elapsed().as_secs(), counts);
                }
                if !hd && counts.iter().all(|count| *count >= 30) {
                    println!("Quatro SD confirmados; mudando cada decoder para HD.");
                    for number in 0..4 {
                        channels[number] = None;
                        let tunnel = client
                            .tunnel(
                                &profile.serial,
                                &service.tunnel_password,
                                &service.remote_ip,
                                554,
                                Duration::from_secs(20),
                            )
                            .await?;
                        let uri = sim_media::dahua_rtsp_url_quality(
                            tunnel.endpoint(),
                            &profile.username,
                            &profile.password,
                            number as u16,
                            sim_media::StreamQuality::Hd,
                        )?;
                        let player = sim_media::Player::start(uri, frames[number].clone(), 1);
                        channels[number] = Some((player, tunnel));
                    }
                    counts = [0; 4];
                    hd = true;
                } else if hd && !closed && counts.iter().all(|count| *count >= 30) {
                    channels[1] = None;
                    closed = true;
                    println!("Decoder 2 encerrado; controle e demais vídeos devem continuar.");
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            stop.store(true, Ordering::Release);
            keep.await??;
            assert!(closed);
            Ok::<_, Box<dyn std::error::Error>>(())
        })
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Exemplo exclusivo para Linux.");
}
