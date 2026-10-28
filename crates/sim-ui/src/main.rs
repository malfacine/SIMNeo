#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod calendar;
mod device_form;
mod export_name;
mod platform;
use platform::file_dialog;
mod layouts;
mod mosaic;
mod network;
mod playback;
mod preferences;
#[cfg(windows)]
use platform::startup;
mod video;
use sim_core::{Channel, ConnectionMode, Device, DeviceDraft, Manufacturer};
use sim_storage_import::{NewDevice, Store, import_simnext_zip};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    error::Error,
    path::PathBuf,
    rc::Rc,
    sync::mpsc::{self, SyncSender},
    time::Duration,
};

slint::include_modules!();

enum Command {
    Save(NewDevice),
    Update(i64, NewDevice),
    Delete(i64),
    Import {
        path: String,
        key: sim_storage_import::ImportKey,
    },
    Sync {
        id: i64,
        channels: Vec<Channel>,
    },
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut path = sim_core::platform::data_directory()?.join("simneo.db");
    let mut smoke = false;
    let mut video_smoke = false;
    let mut mosaic_smoke = false;
    let mut playback_smoke = false;
    let mut gpu_smoke = false;
    let mut restore_smoke = false;
    let mut connection_smoke = false;
    let mut settings = network::Settings {
        sdk: platform::p2p_library_override(),
        coordinator: std::env::var_os("SIMNEO_P2P_CONFIG").map(PathBuf::from),
    };
    // Configuração local ao executável; caminhos relativos usam a pasta do EXE.
    #[derive(serde::Deserialize)]
    struct LocalSettings {
        #[serde(rename = "p2pDll", alias = "p2pLibrary")]
        sdk: Option<PathBuf>,
        #[serde(rename = "p2pConfig")]
        coordinator: Option<PathBuf>,
    }
    if let Some(folder) = std::env::current_exe()?.parent() {
        let file = folder.join("settings.json");
        if file.is_file() {
            let local: LocalSettings = serde_json::from_slice(&std::fs::read(file)?)?;
            let absolute = |path: PathBuf| {
                if path.is_absolute() {
                    path
                } else {
                    folder.join(path)
                }
            };
            if settings.sdk.is_none() {
                settings.sdk = local.sdk.map(absolute);
            }
            if settings.coordinator.is_none() {
                settings.coordinator = local.coordinator.map(absolute);
            }
        }
    }
    if settings.sdk.is_none() {
        settings.sdk = platform::find_p2p_library()?;
    }
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--db" => path = args.next().ok_or("Informe o caminho após --db.")?.into(),
            "--smoke-test" => smoke = true,
            "--connection-smoke-test" => connection_smoke = true,
            "--video-smoke-test" => video_smoke = true,
            "--mosaic-smoke-test" => mosaic_smoke = true,
            "--playback-smoke-test" => playback_smoke = true,
            "--gpu-video-smoke-test" => {
                video_smoke = true;
                gpu_smoke = true;
            }
            "--restore-video-smoke-test" => {
                video_smoke = true;
                restore_smoke = true;
            }
            "--p2p-sdk" => {
                settings.sdk = Some(args.next().ok_or("Informe o caminho do SDK.")?.into())
            }
            "--p2p-config" => {
                settings.coordinator = Some(
                    args.next()
                        .ok_or("Informe o caminho do coordenador.")?
                        .into(),
                )
            }
            _ => {
                return Err(
                    "Argumento desconhecido. Uso: sim-ui [--db caminho] [--smoke-test]".into(),
                );
            }
        }
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    path = std::path::absolute(path)?;
    let pref_store = Store::open(&path)?;
    let loaded: preferences::Preferences = match pref_store.setting("preferences")? {
        Some(json) => {
            serde_json::from_str(&json).map_err(|_| "As preferências salvas são inválidas.")?
        }
        None => Default::default(),
    };
    let preferences = Rc::new(RefCell::new(loaded));
    drop(pref_store);
    let ui = AppWindow::new()?;
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    ui.on_choose_layout({
        let weak = ui.as_weak();
        move |preset| {
            if let Some(ui) = weak.upgrade() {
                layouts::apply(&ui, preset.max(0) as usize);
                ui.set_tile_expanded(false);
                ui.invoke_view_changed();
            }
        }
    });
    ui.set_option_restore(preferences.borrow().restore);
    ui.set_option_gpu(preferences.borrow().gpu);
    ui.set_startup_supported(cfg!(windows));
    #[cfg(windows)]
    ui.set_option_startup(startup::enabled(&std::env::current_exe()?, &path)?);
    let db_path = path.clone();
    let (tx, rx) = mpsc::sync_channel::<Command>(8);
    let (results_tx, results_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut store = match Store::open(&path) {
            Ok(store) => store,
            Err(error) => {
                let _ = results_tx.send(Err(error));
                return;
            }
        };
        let _ = results_tx.send(
            store
                .list()
                .map(|list| (list, "Cadastro local pronto.".to_string(), false)),
        );
        while let Ok(command) = rx.recv() {
            let mutation = !matches!(&command, Command::Sync { .. });
            let deleting = matches!(&command, Command::Delete(_));
            let outcome = match command {
                Command::Save(device) => store.insert_batch(&[device]),
                Command::Update(id, device) => store.update(id, &device),
                Command::Delete(id) => store.delete(id),
                Command::Import { path, key } => {
                    // Limita a leitura antes da alocação; não inclui o conteúdo
                    // do arquivo nem a chave nas mensagens de erro.
                    import_simnext_zip(std::path::Path::new(&path), key.as_str())
                        .and_then(|devices| store.insert_batch(&devices))
                }
                Command::Sync { id, channels } => store.sync_channels(id, &channels).map(|()| 0),
            };
            let result = outcome.and_then(|count| {
                store.list().map(|list| {
                    (
                        list,
                        if deleting {
                            "Dispositivo excluído do cadastro.".into()
                        } else if count == 0 {
                            "Canais sincronizados; nomes locais preservados.".into()
                        } else {
                            format!("{count} dispositivo(s) salvo(s).")
                        },
                        mutation,
                    )
                })
            });
            if results_tx.send(result).is_err() {
                break;
            }
        }
    });

    let devices = Rc::new(RefCell::new(Vec::<Device>::new()));
    let filtered = Rc::new(RefCell::new(Vec::<Device>::new()));
    let initialized = Rc::new(Cell::new(false));
    let (network_tx, network_rx) = mpsc::channel();
    let video = Rc::new(video::Controller::new(
        &ui,
        db_path.clone(),
        settings,
        network_tx,
    ));
    video.set_gpu(
        &ui,
        if gpu_smoke {
            true
        } else {
            preferences.borrow().gpu
        },
    );
    let playback = playback::Controller::new(&ui, video.clone());
    playback.wire(&ui);
    ui.on_save_options({
        let weak = ui.as_weak();
        let preferences = preferences.clone();
        let video = video.clone();
        let devices = devices.clone();
        let db = db_path.clone();
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let result = (|| -> Result<(), Box<dyn Error>> {
                #[cfg(windows)]
                let exe = std::env::current_exe()?;
                #[cfg(windows)]
                let old_startup = startup::enabled(&exe, &db)?;
                let next = preferences::Preferences {
                    restore: ui.get_option_restore(),
                    startup: cfg!(windows) && ui.get_option_startup(),
                    gpu: ui.get_option_gpu(),
                    view: video.snapshot(&ui, &devices.borrow()),
                };
                let store = Store::open(&db)?;
                let json = serde_json::to_string(&next)?;
                #[cfg(windows)]
                if next.startup != old_startup {
                    startup::set(next.startup, &exe, &db)?;
                }
                if let Err(error) = store.set_setting("preferences", &json) {
                    #[cfg(windows)]
                    if next.startup != old_startup {
                        startup::set(old_startup, &exe, &db)?;
                    }
                    return Err(error.into());
                }
                video.set_gpu(&ui, next.gpu);
                *preferences.borrow_mut() = next;
                Ok(())
            })();
            ui.set_options_message(match result {
                Ok(()) => "Opções salvas. A visualização atual será lembrada ao fechar.".into(),
                Err(error) => format!("Não foi possível salvar as opções: {error}").into(),
            });
        }
    });
    let mosaic_verified = Rc::new(Cell::new(false));
    let tester = Rc::new(device_form::Tester::default());
    ui.on_test_device({
        let weak = ui.as_weak();
        let tester = tester.clone();
        let video = video.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                tester.start(&ui, &video);
            }
        }
    });
    ui.on_cancel_test({
        let weak = ui.as_weak();
        let tester = tester.clone();
        let video = video.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                tester.cancel(&ui, &video);
            }
        }
    });
    ui.on_filter({
        let weak = ui.as_weak();
        let devices = devices.clone();
        let filtered = filtered.clone();
        let video = video.clone();
        move |query| {
            if let Some(ui) = weak.upgrade() {
                refresh(&ui, &devices.borrow(), &filtered, query.as_str());
                video.decorate(&ui);
            }
        }
    });
    ui.on_select_device({
        let weak = ui.as_weak();
        let filtered = filtered.clone();
        let video = video.clone();
        move |index| {
            if let Some(ui) = weak.upgrade() {
                select(&ui, &filtered.borrow(), index);
                video.selected_status(&ui);
            }
        }
    });
    ui.on_save_device({
        let weak = ui.as_weak();
        let tx = tx.clone();
        let filtered = filtered.clone();
        let video = video.clone();
        let tester = tester.clone();
        move |name, address, port, username, password, maker, count, connection_mode, serial| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !(0..=2).contains(&connection_mode) {
                ui.set_status("Marque IP, serial ou ambos para continuar.".into());
                return;
            }
            let parsed = port
                .trim()
                .parse::<u16>()
                .ok()
                .zip(count.trim().parse::<u16>().ok());
            let Some((port, count)) =
                parsed.filter(|(port, count)| *port > 0 && (1..=1024).contains(count))
            else {
                ui.set_status("Confira a porta (1–65535) e o número de canais (1–1024).".into());
                return;
            };
            let editing = ui.get_edit_id().parse::<i64>().ok();
            let existing =
                editing.and_then(|id| filtered.borrow().iter().find(|d| d.id == id).cloned());
            let mut config = DeviceDraft {
                serial: if connection_mode == 2 {
                    Some(serial.trim().into())
                } else {
                    None
                },
                connection: if connection_mode == 1 {
                    ConnectionMode::P2p
                } else {
                    ConnectionMode::Address
                },
                name: name.trim().into(),
                address: if connection_mode == 1 {
                    serial.trim().into()
                } else {
                    address.trim().into()
                },
                port,
                username: username.trim().into(),
                manufacturer: if maker == 0 {
                    Manufacturer::Dahua
                } else {
                    Manufacturer::Jufeng
                },
                channels: (0..count)
                    .map(|number| Channel {
                        number,
                        name: existing
                            .as_ref()
                            .and_then(|d| d.config.channels.iter().find(|c| c.number == number))
                            .map(|c| c.name.clone())
                            .unwrap_or_else(|| format!("Canal {:02}", number + 1)),
                    })
                    .collect(),
            };
            if let Some(channels) = tester.channels(&ui, count) {
                config.channels = channels
                    .into_iter()
                    .map(|mut c| {
                        if let Some(local) = existing.as_ref().and_then(|d| {
                            d.config
                                .channels
                                .iter()
                                .find(|local| local.number == c.number)
                        }) {
                            c.name = local.name.clone();
                        }
                        c
                    })
                    .collect();
            }
            if let Err(error) = config.validate() {
                ui.set_status(error.to_string().into());
                return;
            }
            let device = NewDevice::new(config, password.to_string());
            tester.cancel(&ui, &video);
            let command = if let Some(id) = editing {
                video.disconnect(&ui, id);
                Command::Update(id, device)
            } else {
                Command::Save(device)
            };
            dispatch(&ui, &tx, command);
        }
    });
    ui.on_edit_device({
        let weak = ui.as_weak();
        let filtered = filtered.clone();
        let tester = tester.clone();
        let video = video.clone();
        move || {
            if let Some(ui) = weak.upgrade()
                && let Some(d) = usize::try_from(ui.get_selected_index())
                    .ok()
                    .and_then(|i| filtered.borrow().get(i).cloned())
            {
                tester.cancel(&ui, &video);
                ui.set_edit_id(d.id.to_string().into());
                ui.set_form_name(d.config.name.into());
                ui.set_form_address(if d.config.connection == ConnectionMode::P2p {
                    "".into()
                } else {
                    d.config.address.clone().into()
                });
                ui.set_form_serial(if d.config.connection == ConnectionMode::P2p {
                    d.config.address.into()
                } else {
                    d.config.serial.clone().unwrap_or_default().into()
                });
                ui.set_form_port(d.config.port.to_string().into());
                ui.set_form_user(d.config.username.into());
                ui.set_form_password("".into());
                ui.set_form_count(d.config.channels.len().max(1).to_string().into());
                ui.set_form_maker(if d.config.manufacturer == Manufacturer::Dahua {
                    0
                } else {
                    1
                });
                ui.set_form_use_ip(d.config.connection == ConnectionMode::Address);
                ui.set_form_use_serial(
                    d.config.connection == ConnectionMode::P2p || d.config.serial.is_some(),
                );
                ui.set_dialog("device".into());
            }
        }
    });
    ui.on_delete_device({
        let weak = ui.as_weak();
        let tx = tx.clone();
        let video = video.clone();
        move || {
            if let Some(ui) = weak.upgrade()
                && let Ok(id) = ui.get_selected_id().parse::<i64>()
            {
                video.disconnect(&ui, id);
                dispatch(&ui, &tx, Command::Delete(id));
            }
        }
    });
    ui.on_browse_zip({
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            ui.set_dialog("picking".into());
            let owner = platform::foreground_owner();
            let weak = ui.as_weak();
            std::thread::spawn(move || {
                let picked = file_dialog::pick_zip(owner);
                let _ = weak.upgrade_in_event_loop(move |ui| match picked {
                    Ok(Some(path)) => {
                        ui.set_import_path(path.to_string_lossy().into_owned().into());
                        ui.set_zip_password("".into());
                        ui.set_dialog("import".into());
                    }
                    Ok(None) => ui.set_dialog("".into()),
                    Err(message) => {
                        ui.set_dialog("".into());
                        ui.set_status(message.into());
                    }
                });
            });
        }
    });
    ui.on_connect_device({
        let weak = ui.as_weak();
        let filtered = filtered.clone();
        let video = video.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let device = usize::try_from(ui.get_selected_index())
                    .ok()
                    .and_then(|i| filtered.borrow().get(i).cloned());
                if let Some(device) = device {
                    video.connect(&ui, device);
                }
            }
        }
    });
    ui.on_play_video({
        let weak = ui.as_weak();
        let filtered = filtered.clone();
        let video = video.clone();
        move |channel| {
            if let Some(ui) = weak.upgrade() {
                let device = usize::try_from(ui.get_selected_index())
                    .ok()
                    .and_then(|i| filtered.borrow().get(i).cloned());
                if let (Some(device), Ok(channel), Ok(slot)) = (
                    device,
                    u16::try_from(channel),
                    usize::try_from(ui.get_selected_slot()),
                ) {
                    video.assign(&ui, &device, channel, slot);
                }
            }
        }
    });
    ui.on_open_device({
        let weak = ui.as_weak();
        let filtered = filtered.clone();
        let video = video.clone();
        move |index| {
            if let Some(ui) = weak.upgrade()
                && let Some(device) = usize::try_from(index)
                    .ok()
                    .and_then(|i| filtered.borrow().get(i).cloned())
            {
                video.open_device(&ui, &device);
            }
        }
    });
    ui.on_close_video({
        let weak = ui.as_weak();
        let video = video.clone();
        move |slot| {
            if let (Some(ui), Ok(slot)) = (weak.upgrade(), usize::try_from(slot)) {
                video.close(&ui, slot);
            }
        }
    });
    ui.on_stop_all({
        let weak = ui.as_weak();
        let video = video.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                video.stop_all(&ui);
            }
        }
    });
    ui.on_quality({
        let weak = ui.as_weak();
        let video = video.clone();
        move |mode| {
            if let Some(ui) = weak.upgrade() {
                video.quality(&ui, mode);
            }
        }
    });
    ui.on_view_changed({
        let weak = ui.as_weak();
        let video = video.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                video.view_changed(&ui);
            }
        }
    });
    ui.on_disconnect_device({
        let weak = ui.as_weak();
        let video = video.clone();
        move || {
            if let Some(ui) = weak.upgrade()
                && let Ok(id) = ui.get_selected_id().as_str().parse::<i64>()
            {
                video.disconnect(&ui, id);
            }
        }
    });
    ui.on_import_xml({
        let weak = ui.as_weak();
        let tx = tx.clone();
        move |path, key| {
            if let Some(ui) = weak.upgrade() {
                if path.trim().is_empty() {
                    ui.set_status("Selecione o arquivo ZIP.".into());
                    return;
                }
                dispatch(
                    &ui,
                    &tx,
                    Command::Import {
                        path: path.trim().trim_matches('"').into(),
                        key: sim_storage_import::ImportKey::new(key.to_string()),
                    },
                );
            }
        }
    });

    let timer = slint::Timer::default();
    let mut smoke_started = false;
    let mut smoke_phase = 0u8;
    let mut baseline = [0u64; mosaic::CAPACITY];
    let mut restored = false;
    let playback_verified = Rc::new(Cell::new(false));
    let mut playback_phase = 0u8;
    let mut playback_speed_step = 1;
    let mut playback_baseline = 0;
    let mut playback_pause_at = std::time::Instant::now();
    let mut playback_pause_clock = String::new();
    let connection_verified = Rc::new(Cell::new(0u16));
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), {
        let weak=ui.as_weak();let initialized=initialized.clone();let tx=tx.clone();
        let video=video.clone();let verified=mosaic_verified.clone();
        let tester=tester.clone();
        let connection_verified=connection_verified.clone();
        let preferences=preferences.clone();let devices=devices.clone();let playback=playback.clone();let playback_verified=playback_verified.clone();
        move||{
            let Some(ui)=weak.upgrade()else{return;};
            while let Ok(event)=network_rx.try_recv(){
                if let network::Event::Archive{token,update}=event {
                    #[cfg(target_os = "linux")]
                    if playback_smoke {
                        match &update {
                            network::archive::Update::Error(message) | network::archive::Update::SlotError{message,..} => eprintln!("SIMNeo Linux: playback · {message}"),
                            _ => {}
                        }
                    }
                    playback.event(&ui,token,update);continue;
                }
                if let network::Event::Probe{token,result}=event {tester.result(&ui,token,result);continue;}
                #[cfg(target_os = "linux")]
                if mosaic_smoke && let network::Event::Video{slot,state,message,..}=&event {
                    println!("SIMNeo Linux: posição {} · {state:?} · {message}",slot+1);
                }
                #[cfg(target_os = "linux")]
                if (mosaic_smoke || video_smoke) && let network::Event::Device{state,message,..}=&event {
                    println!("SIMNeo Linux: dispositivo · {state:?} · {message}");
                }
                if (video_smoke||mosaic_smoke) && let network::Event::Device{message,..}=&event
                    && (message.starts_with("Conectado por IP") || message.starts_with("Conectado pelo serial") || message.starts_with("Conexão por IP falhou")) {
                    println!("SIMNeo: {message}");
                }
                if let Some((id,channels))=video.event(&ui,event) && tx.try_send(Command::Sync{id,channels}).is_err(){ui.set_status("Não foi possível agendar a sincronização dos canais.".into());}
            }
            while let Ok(result)=results_rx.try_recv(){
                ui.set_busy(false);
                match result{
                    Ok((list,message,mutation))=>{initialized.set(true);*devices.borrow_mut()=list;
                        refresh(&ui,&devices.borrow(),&filtered,ui.get_search().as_str());video.decorate(&ui);
                        playback.refresh(&ui,&devices.borrow());
                        if mutation {ui.set_dialog("".into());ui.set_form_password("".into());ui.set_zip_password("".into());}ui.set_status(message.into());
                    }
                    Err(error)=>ui.set_status(error.to_string().into()),
                }
            }
            video.poll();playback.poll(&ui);
            if initialized.get() && !restored {
                restored=true;
                let preferences=preferences.borrow();
                if preferences.restore && !(smoke||connection_smoke||mosaic_smoke||playback_smoke||(video_smoke && !restore_smoke)){video.restore(&ui,&devices.borrow(),&preferences.view);}
            }
            if connection_smoke && initialized.get() && !smoke_started {
                smoke_started=true;ui.invoke_edit_device();ui.set_form_count("1".into());ui.invoke_test_device();
            }
            if connection_smoke && smoke_started && !ui.get_testing_device() {
                if ui.get_probe_message().starts_with("Conexão testada") && let Ok(count)=ui.get_form_count().parse::<u16>()
                    && tester.channels(&ui,count).is_some() {connection_verified.set(count);}
                let _=slint::quit_event_loop();
            }
            if (video_smoke||mosaic_smoke) && initialized.get() && !smoke_started {
                smoke_started=true;
                if mosaic_smoke {
                    ui.set_layout_size(4);ui.invoke_open_device(ui.get_selected_index());smoke_phase=1;
                }else if !restore_smoke {ui.invoke_play_video(ui.get_video_channel());}
            }
            if playback_smoke && initialized.get() {
                match playback_phase {
                    0=>{ui.set_page("playback".into());playback.toggle(&ui,0,true);playback.toggle(&ui,1,true);assert_eq!(ui.get_playback_channels().iter().filter(|c|c.checked).map(|c|c.number).collect::<Vec<_>>(),vec![1]);playback.toggle(&ui,1,false);assert!(!ui.get_playback_channels().iter().any(|c|c.checked));playback.toggle(&ui,1,true);assert_eq!(ui.get_playback_tiles().row_count(),1);println!("SIMNeo: seleção exclusiva e uma única posição de reprodução validadas.");ui.set_playback_date("2026-10-26".into());ui.set_playback_to("00:02:00".into());ui.invoke_search_playback();playback_phase=1;}
                    1 if ui.get_playback_ready() && ui.get_playback_running() && !ui.get_playback_busy()=>{println!("SIMNeo: busca iniciou reprodução automaticamente.");playback_phase=2;}
                    2 if playback.displayed.get()>=40=>{ui.invoke_play_pause();playback_pause_at=std::time::Instant::now();playback_pause_clock=ui.get_playback_clock().to_string();playback_phase=3;}
                    3 if playback_pause_at.elapsed()>=Duration::from_secs(1) && ui.get_playback_paused()=>{if ui.get_playback_clock()==playback_pause_clock.as_str(){println!("SIMNeo: pausa preservou posição; buscando metade do período.");ui.invoke_seek_playback(0.5);playback_baseline=playback.displayed.get();playback_phase=4;}else{playback_pause_clock=ui.get_playback_clock().to_string();playback_pause_at=std::time::Instant::now();}}
                    4 if playback.displayed.get()>=playback_baseline+30 && ui.get_playback_clock().as_str()>="00:01:00"=>{println!("SIMNeo: seek real exibiu quadros após 00:01:00; exportando trecho MP4.");ui.invoke_prepare_export();assert_eq!(ui.get_export_quality(),0);ui.set_export_from("00:00:03".into());ui.set_export_to("00:00:13".into());ui.invoke_export_range_edited();assert!(ui.get_export_filename().ends_with("-2026-10-26-00-00-03-00-00-13.mp4"));assert_eq!(ui.get_export_filename(),ui.get_export_path());println!("SIMNeo: nome automático acompanha o período; qualidade HD selecionada.");ui.set_export_path(platform::playback_smoke_path().into());ui.invoke_export_playback();playback_phase=5;}
                    5 if !ui.get_export_busy() && ui.get_export_message().starts_with("MP4 salvo:")=>{println!("SIMNeo: playback, pausa, seek e exportação MP4 validados na interface.");ui.invoke_playback_speed_changed(1);ui.invoke_seek_playback(0.0);playback_baseline=playback.displayed.get();playback_phase=7;}
                    7 if ui.get_playback_running() && !ui.get_playback_busy() && playback.displayed.get()>playback_baseline+2=>{playback_pause_at=std::time::Instant::now();playback_pause_clock=ui.get_playback_clock().to_string();playback_phase=8;}
                    8 if playback_pause_at.elapsed()>=Duration::from_millis(1500)=>{let second=|s:&str| sim_core::recording::Stamp::parse(&format!("2026-10-26 {s}")).unwrap().second();let advanced=second(ui.get_playback_clock().as_str()).saturating_sub(second(&playback_pause_clock));let rate=1u32<<playback_speed_step;println!("SIMNeo: velocidade {rate}x avançou {advanced}s em {:.2}s.",playback_pause_at.elapsed().as_secs_f64());assert!(f64::from(advanced)>=f64::from(rate)*playback_pause_at.elapsed().as_secs_f64()*0.5);if playback_speed_step==4{playback_verified.set(true);playback_phase=6;let _=slint::quit_event_loop();}else{playback_speed_step+=1;ui.invoke_playback_speed_changed(playback_speed_step);ui.invoke_seek_playback(0.0);playback_baseline=playback.displayed.get();playback_phase=7;}}
                    _=>{}
                }
            }
            let counts=video.displayed.get();
            if video_smoke && counts[0]>=30{let _=slint::quit_event_loop();}
            if mosaic_smoke {
                match smoke_phase {
                    1 if counts[..4].iter().all(|n|*n>=20)=>{println!("SIMNeo: quatro canais SD exibidos; mudando todos para HD.");ui.invoke_quality(1);smoke_phase=2;}
                    2 if counts[..4].iter().all(|n|*n>=20) && (0..4).all(|i|video.details(i).ends_with(" · HD"))=>{ui.set_selected_slot(0);ui.set_tile_expanded(true);ui.invoke_view_changed();println!("SIMNeo: quatro canais HD; ampliação mantém qualidade: {}; fechando somente posição 2.",video.status(0));ui.invoke_close_video(1);baseline=counts;smoke_phase=3;}
                    3 if counts[2]>=baseline[2]+10 && counts[3]>=baseline[3]+10=>{
                        println!("SIMNeo: demais canais continuam recebendo frames; substituindo posição 2.");ui.invoke_quality(0);ui.set_tile_expanded(false);ui.invoke_view_changed();ui.set_selected_slot(1);ui.invoke_play_video(4);smoke_phase=4;
                    }
                    4 if counts[..4].iter().all(|n|*n>=20) && (0..4).all(|i|video.details(i).ends_with(" · SD"))=>{baseline=counts;ui.invoke_change_layout(1);println!("SIMNeo: layout reduzido de 4 para 1; verificando posições preservadas.");smoke_phase=5;}
                    5 if counts[..4].iter().zip(baseline).all(|(n,b)|*n>=b+10)=>{let saved=video.snapshot(&ui,&devices.borrow());if saved.layout==1 && saved.slots.iter().flatten().count()==4 {ui.invoke_change_layout(4);baseline=counts;smoke_phase=6;}}
                    6 if counts[..4].iter().zip(baseline).all(|(n,b)|*n>=b+10) && ui.get_active_videos()==4=>{let saved=video.snapshot(&ui,&devices.borrow());if saved.slots.iter().flatten().count()==4 {println!("SIMNeo: retorno 1 para 4 preservou as quatro câmeras e o estado salvo.");verified.set(true);smoke_phase=7;let _=slint::quit_event_loop();}}
                    _=>{}
                }
            }
        }
    });
    let smoke_timer = slint::Timer::default();
    if smoke || connection_smoke || video_smoke || mosaic_smoke || playback_smoke {
        smoke_timer.start(
            slint::TimerMode::SingleShot,
            if playback_smoke {
                Duration::from_secs(180)
            } else if mosaic_smoke {
                Duration::from_secs(120)
            } else if connection_smoke {
                Duration::from_secs(60)
            } else if video_smoke {
                Duration::from_secs(45)
            } else {
                Duration::from_millis(1500)
            },
            || {
                let _ = slint::quit_event_loop();
            },
        );
    }
    ui.run()?;
    let snapshot = video.snapshot(&ui, &devices.borrow());
    preferences.borrow_mut().view = snapshot;
    let save_result = if !(smoke
        || connection_smoke
        || (video_smoke && !restore_smoke)
        || mosaic_smoke
        || playback_smoke)
    {
        Store::open(&db_path).and_then(|store| {
            store.set_setting(
                "preferences",
                &serde_json::to_string(&*preferences.borrow()).expect("preferências serializáveis"),
            )
        })
    } else {
        Ok(())
    };
    let last_status = video.status(0);
    let displayed = video.displayed.get();
    drop(timer);
    // Remove closures that own sender clones before joining the worker.
    drop(ui);
    drop(playback);
    drop(video);
    drop(tx);
    worker
        .join()
        .map_err(|_| "A tarefa de armazenamento foi interrompida.")?;
    save_result?;
    if playback_smoke && !playback_verified.get() {
        return Err("Playback não concluiu busca/pausa/seek/exportação no prazo.".into());
    }
    if connection_smoke {
        if connection_verified.get() == 0 {
            return Err("O teste do formulário não encontrou canais.".into());
        }
        println!(
            "SIMNeo: teste de cadastro concluído; {} canais preenchidos no formulário; nenhum cadastro alterado.",
            connection_verified.get()
        );
    }
    if smoke && !initialized.get() {
        return Err("A inicialização do cadastro falhou.".into());
    }
    if smoke {
        println!("SIMNeo: janela, event loop e SQLite inicializados.");
    }
    if video_smoke {
        if gpu_smoke && !last_status.contains("GPU D3D11VA") {
            return Err("O teste não confirmou decodificação por GPU.".into());
        }
        if displayed[0] < 30 {
            return Err("O teste não exibiu 30 frames no prazo.".into());
        }
        println!(
            "SIMNeo: {} frames exibidos no Slint; sessão e decoder encerrados.",
            displayed[0]
        );
        if gpu_smoke {
            println!("SIMNeo: backend confirmado: {last_status}");
        }
    }
    if mosaic_smoke {
        if !mosaic_verified.get() {
            return Err("O teste de quatro canais/HD/substituição não concluiu no prazo.".into());
        }
        println!(
            "SIMNeo: quatro canais simultâneos, qualidade global SD/HD, ampliação, fechamento isolado e substituição validados; recursos encerrados."
        );
    }
    Ok(())
}

fn dispatch(ui: &AppWindow, tx: &SyncSender<Command>, command: Command) {
    if tx.try_send(command).is_ok() {
        ui.set_busy(true);
        ui.set_status("Salvando cadastro local…".into());
    } else {
        ui.set_status("Armazenamento indisponível. Reinicie o aplicativo.".into());
    }
}

fn refresh(ui: &AppWindow, devices: &[Device], filtered: &RefCell<Vec<Device>>, query: &str) {
    let selected_id = usize::try_from(ui.get_selected_index())
        .ok()
        .and_then(|i| filtered.borrow().get(i).map(|d| d.id));
    let query = query.trim().to_lowercase();
    let list: Vec<Device> = devices
        .iter()
        .filter(|d| {
            d.config.name.to_lowercase().contains(&query)
                || d.config.address.to_lowercase().contains(&query)
                || d.config
                    .serial
                    .as_ref()
                    .is_some_and(|s| s.to_lowercase().contains(&query))
        })
        .cloned()
        .collect();
    let rows: Vec<DeviceRow> = list
        .iter()
        .map(|d| DeviceRow {
            id: d.id.to_string().into(),
            name: d.config.name.clone().into(),
            endpoint: if let Some(serial) = &d.config.serial {
                format!("{}:{} · Serial {}", d.config.address, d.config.port, serial)
            } else {
                format!("{}:{}", d.config.address, d.config.port)
            }
            .into(),
            summary: format!("{} canais", d.config.channels.len()).into(),
            state: 0,
            state_text: "Desconectado".into(),
        })
        .collect();
    ui.set_devices(ModelRc::new(VecModel::from(rows)));
    ui.set_total(
        format!(
            "{} dispositivos · {} canais",
            devices.len(),
            devices
                .iter()
                .map(|d| d.config.channels.len())
                .sum::<usize>()
        )
        .into(),
    );
    let selected = selected_id
        .and_then(|id| list.iter().position(|d| d.id == id))
        .map(|i| i as i32)
        .unwrap_or(if list.is_empty() { -1 } else { 0 });
    select(ui, &list, selected);
    *filtered.borrow_mut() = list;
}

fn select(ui: &AppWindow, devices: &[Device], index: i32) {
    ui.set_selected_index(index);
    if let Some(device) = usize::try_from(index).ok().and_then(|i| devices.get(i)) {
        ui.set_selected_id(device.id.to_string().into());
        ui.set_selected_name(device.config.name.clone().into());
        ui.set_selected_endpoint(
            format!("{}:{}", device.config.address, device.config.port).into(),
        );
        ui.set_selected_manufacturer(
            match device.config.manufacturer {
                Manufacturer::Dahua => "Intelbras / Dahua",
                Manufacturer::Jufeng => "Jufeng / Xiongmai",
            }
            .into(),
        );
        let channels: Vec<ChannelRow> = device
            .config
            .channels
            .iter()
            .map(|c| ChannelRow {
                number: i32::from(c.number),
                name: format!("{:02}   {}", u32::from(c.number) + 1, c.name).into(),
            })
            .collect();
        ui.set_channels(ModelRc::new(VecModel::from(channels)));
    } else {
        ui.set_selected_id("".into());
        ui.set_selected_name("".into());
        ui.set_selected_endpoint("".into());
        ui.set_selected_manufacturer("".into());
        ui.set_channels(ModelRc::default());
    }
}
