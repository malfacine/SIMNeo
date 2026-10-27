use crate::mosaic::{CAPACITY, retry_delay};
use serde::Deserialize;
use sim_core::{Channel, ConnectionMode, Device, Manufacturer};
use sim_storage_import::{Store, read_protected_file};
use std::{
    collections::HashMap, error::Error, path::PathBuf, rc::Rc, sync::mpsc::Sender,
    thread::JoinHandle, time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    task::{AbortHandle, JoinSet, LocalSet},
};
use zeroize::Zeroize;
pub mod archive;
mod players;
mod rtsp_speed;
#[cfg(test)]
mod tests;

type Failure = Box<dyn Error>;
include!(concat!(env!("OUT_DIR"), "/p2p_service.rs"));
pub type Frames = [sim_media::FrameSlot; CAPACITY];
type Requests = [Option<VideoRequest>; CAPACITY];

#[derive(Clone, Default)]
pub struct Settings {
    pub sdk: Option<PathBuf>,
    pub coordinator: Option<PathBuf>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceState {
    Offline,
    Connecting,
    Online,
    Reconnecting,
    Error,
}
impl DeviceState {
    pub fn active(self) -> bool {
        matches!(self, Self::Connecting | Self::Online | Self::Reconnecting)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoState {
    Connecting,
    Playing,
    Reconnecting,
    Error,
}
pub enum Event {
    Archive {
        token: u64,
        update: archive::Update,
    },
    Probe {
        token: u64,
        result: Result<(Vec<Channel>, String), String>,
    },
    Device {
        id: i64,
        state: DeviceState,
        message: String,
    },
    Online {
        id: i64,
        channels: Vec<Channel>,
    },
    Video {
        slot: usize,
        generation: u64,
        state: VideoState,
        message: String,
        details: Option<String>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoRequest {
    pub channel: u16,
    pub generation: u64,
    pub quality: sim_media::StreamQuality,
    pub fallback: bool,
    pub gpu: bool,
}
enum Command {
    Archive(u64, Box<(Device, archive::Job)>),
    CancelArchive(bool),
    Probe(u64, sim_storage_import::NewDevice, Option<i64>),
    CancelProbe(u64),
    Connect(Device),
    Disconnect(i64),
    Play(usize, i64, VideoRequest),
    Stop(usize),
    StopAll,
}
pub struct Handle {
    cancel: watch::Sender<bool>,
    commands: mpsc::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}
impl Handle {
    pub fn archive(&self, token: u64, device: Device, job: archive::Job) -> bool {
        self.commands
            .try_send(Command::Archive(token, Box::new((device, job))))
            .is_ok()
    }
    pub fn cancel_archive(&self, export: bool) {
        let _ = self.commands.try_send(Command::CancelArchive(export));
    }
    pub fn probe(
        &self,
        token: u64,
        device: sim_storage_import::NewDevice,
        existing: Option<i64>,
    ) -> bool {
        self.commands
            .try_send(Command::Probe(token, device, existing))
            .is_ok()
    }
    pub fn cancel_probe(&self, token: u64) {
        let _ = self.commands.try_send(Command::CancelProbe(token));
    }
    pub fn connect(&self, device: Device) -> bool {
        self.commands.try_send(Command::Connect(device)).is_ok()
    }
    pub fn disconnect(&self, id: i64) -> bool {
        self.commands.try_send(Command::Disconnect(id)).is_ok()
    }
    pub fn play(&self, slot: usize, id: i64, request: VideoRequest) -> bool {
        slot < CAPACITY
            && self
                .commands
                .try_send(Command::Play(slot, id, request))
                .is_ok()
    }
    pub fn stop_video(&self, slot: usize) -> bool {
        slot < CAPACITY && self.commands.try_send(Command::Stop(slot)).is_ok()
    }
    pub fn stop_all(&self) -> bool {
        self.commands.try_send(Command::StopAll).is_ok()
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Coordinator {
    server: String,
    port: u16,
    key: String,
    tunnel_password: String,
    remote_ip: String,
}
impl Coordinator {
    fn load(path: &std::path::Path) -> Result<Self, Failure> {
        let json = if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            zeroize::Zeroizing::new(std::fs::read_to_string(path)?)
        } else {
            read_protected_file(path)?
        };
        Self::parse(&json)
    }
    fn configured(path: Option<&std::path::Path>, embedded: Option<&str>) -> Result<Self, Failure> {
        if let Some(path) = path {
            return Self::load(path);
        }
        Self::parse(embedded.ok_or("Configuração do serviço P2P ausente.")?)
    }
    fn parse(json: &str) -> Result<Self, Failure> {
        let config: Self =
            serde_json::from_str(json).map_err(|_| "Configuração do serviço P2P inválida.")?;
        if config.server.trim().is_empty()
            || config.port == 0
            || config.key.is_empty()
            || config.tunnel_password.is_empty()
            || config.remote_ip.trim().is_empty()
        {
            return Err("Configuração do serviço P2P incompleta.".into());
        }
        Ok(config)
    }
}
impl Drop for Coordinator {
    fn drop(&mut self) {
        self.key.zeroize();
        self.tunnel_password.zeroize();
    }
}
struct Relay {
    client: sim_p2p_t2u::Client,
    config: Coordinator,
}
impl Relay {
    fn load(settings: &Settings) -> Result<Self, Failure> {
        let config =
            Coordinator::configured(settings.coordinator.as_deref(), EMBEDDED_P2P_SERVICE)?;
        let sdk = settings.sdk.as_ref().ok_or("Configure o SDK P2P.")?;
        let client = sim_p2p_t2u::Client::load(sdk, &config.server, config.port, &config.key)?;
        Ok(Self { client, config })
    }
    async fn tunnel(
        &self,
        serial: &str,
        port: u16,
    ) -> Result<sim_p2p_t2u::Tunnel<'_>, sim_p2p_t2u::Error> {
        self.client
            .tunnel(
                serial,
                &self.config.tunnel_password,
                &self.config.remote_ip,
                port,
                Duration::from_secs(20),
            )
            .await
    }
}
// Compartilhado exclusivamente pela thread proprietária do SDK; carregado sob demanda.
struct RelayPool {
    settings: Settings,
    loaded: std::cell::RefCell<Option<Rc<Relay>>>,
}
impl RelayPool {
    fn get(&self) -> Result<Rc<Relay>, Failure> {
        if let Some(relay) = self.loaded.borrow().as_ref() {
            return Ok(relay.clone());
        }
        let relay = Rc::new(Relay::load(&self.settings)?);
        self.loaded.replace(Some(relay.clone()));
        Ok(relay)
    }
}
pub fn start(db: PathBuf, settings: Settings, events: Sender<Event>, frames: Frames) -> Handle {
    let (cancel, mut stop) = watch::channel(false);
    let (commands, rx) = mpsc::channel(128);
    let thread = std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(
                LocalSet::new().run_until(manager(db, settings, events, frames, rx, &mut stop)),
            ),
            Err(error) => {
                let _ = events.send(Event::Device {
                    id: 0,
                    state: DeviceState::Error,
                    message: format!("Não foi possível iniciar a rede: {error}"),
                });
            }
        }
    });
    Handle {
        cancel,
        commands,
        thread: Some(thread),
    }
}
struct DeviceTask {
    epoch: u64,
    requests: watch::Sender<Requests>,
    abort: AbortHandle,
}
fn requests_for(id: i64, desired: &[Option<(i64, VideoRequest)>; CAPACITY]) -> Requests {
    std::array::from_fn(|slot| {
        desired[slot]
            .filter(|(owner, _)| *owner == id)
            .map(|(_, r)| r)
    })
}
fn publish(
    id: i64,
    desired: &[Option<(i64, VideoRequest)>; CAPACITY],
    devices: &HashMap<i64, DeviceTask>,
) {
    if let Some(task) = devices.get(&id) {
        let _ = task.requests.send(requests_for(id, desired));
    }
}
async fn manager(
    db: PathBuf,
    settings: Settings,
    events: Sender<Event>,
    frames: Frames,
    mut commands: mpsc::Receiver<Command>,
    stop: &mut watch::Receiver<bool>,
) {
    let mut tasks = JoinSet::new();
    let mut devices: HashMap<i64, DeviceTask> = HashMap::new();
    let mut desired = [None; CAPACITY];
    let mut epoch = 0u64;
    let mut probes = JoinSet::new();
    let mut active_probe: Option<(u64, AbortHandle)> = None;
    let mut archive_tasks = JoinSet::new();
    let mut archive_active: [Option<AbortHandle>; 2] = [None, None];
    let relay = Rc::new(RelayPool {
        settings,
        loaded: Default::default(),
    });
    loop {
        tokio::select! {
            _=stop.changed()=>break,
            _=archive_tasks.join_next(),if !archive_tasks.is_empty()=>{},
            joined=probes.join_next(),if !probes.is_empty()=>{
                match joined {
                    Some(Ok((token,result)))=>{
                        if active_probe.as_ref().is_some_and(|(id,_)|*id==token){active_probe=None;let _=events.send(Event::Probe{token,result});}
                    }
                    Some(Err(error))=>{
                        if active_probe.as_ref().is_some_and(|(_,task)|task.id()==error.id()) && let Some((token,_))=active_probe.take(){
                            let _=events.send(Event::Probe{token,result:Err("A tarefa do teste de conexão foi interrompida.".into())});
                        }
                    }
                    None=>{}
                }
            }
            joined=tasks.join_next(),if !tasks.is_empty()=>{
                if let Some(Ok((id,version,result)))=joined {
                    if devices.get(&id).is_some_and(|d|d.epoch==version){
                        devices.remove(&id);
                        let (state,message)=match result {Ok(())=>(DeviceState::Offline,"Dispositivo desconectado.".into()),Err(e)=>(DeviceState::Error,e)};
                        let _=events.send(Event::Device{id,state,message});
                    }
                } else if let Some(Err(error))=joined
                    && let Some(id)=devices.iter().find_map(|(id,d)|(d.abort.id()==error.id()).then_some(*id)) {
                    devices.remove(&id);
                    let _=events.send(Event::Device{id,state:DeviceState::Error,message:"A tarefa de conexão do DVR foi interrompida.".into()});
                }
            }
            command=commands.recv()=>match command {
                None=>break,
                Some(Command::Archive(token,request))=>{
                    let (device,job)=*request;
                    let lane=usize::from(matches!(job,archive::Job::Export{..}));
                    if let Some(active)=archive_active[lane].take(){active.abort();}
                    let pool=relay.clone();let db=db.clone();let events=events.clone();
                    archive_active[lane]=Some(archive_tasks.spawn_local(async move {
                        if let Err(error)=archive::run(token,device,db,pool,events.clone(),job).await {
                            let _=events.send(Event::Archive{token,update:archive::Update::Error(error.to_string())});
                        }
                    }));
                }
                Some(Command::CancelArchive(export))=>{if let Some(active)=archive_active[usize::from(export)].take(){active.abort();}}
                Some(Command::Probe(token,draft,existing))=>{
                    if let Some((_,task))=active_probe.take(){task.abort();}
                    let pool=relay.clone();let db=db.clone();
                    let task=probes.spawn_local(async move {
                        let result=tokio::time::timeout(Duration::from_secs(50),probe(draft,existing,&db,&pool)).await;
                        let result=match result {Ok(result)=>result.map_err(|e|e.to_string()),Err(_)=>Err("O teste de conexão excedeu o prazo.".into())};
                        (token,result)
                    });active_probe=Some((token,task));
                }
                Some(Command::CancelProbe(token))=>{
                    if active_probe.as_ref().is_some_and(|(id,_)|*id==token) && let Some((_,task))=active_probe.take(){task.abort();}
                }
                Some(Command::Connect(device))=>{
                    let id=device.id;if devices.contains_key(&id){continue;}
                    epoch=epoch.wrapping_add(1);let version=epoch;
                    let (tx,rx)=watch::channel(requests_for(id,&desired));
                    let child_relay=relay.clone();let child_db=db.clone();let child_events=events.clone();let child_frames=frames.clone();
                    let abort=tasks.spawn_local(async move{
                        let result=device_loop(device,child_db,child_relay,child_events,child_frames,rx).await.map_err(|e|e.to_string());
                        (id,version,result)
                    });
                    devices.insert(id,DeviceTask{epoch:version,requests:tx,abort});
                }
                Some(Command::Disconnect(id))=>{
                    for item in &mut desired {if item.is_some_and(|(owner,_)|owner==id){*item=None;}}
                    if let Some(task)=devices.remove(&id){task.abort.abort();}
                    let _=events.send(Event::Device{id,state:DeviceState::Offline,message:"Dispositivo desconectado.".into()});
                }
                Some(Command::Play(slot,id,request))=>{
                    if !devices.contains_key(&id){let _=events.send(Event::Video{slot,generation:request.generation,state:VideoState::Error,message:"Conecte o dispositivo para abrir o canal.".into(),details:None});continue;}
                    let old=desired[slot].map(|(owner,_)|owner);desired[slot]=Some((id,request));
                    if let Some(old)=old.filter(|old|*old!=id){publish(old,&desired,&devices);}
                    publish(id,&desired,&devices);
                }
                Some(Command::Stop(slot))=>{if let Some((id,_))=desired[slot].take(){publish(id,&desired,&devices);}}
                Some(Command::StopAll)=>{desired=[None;CAPACITY];for id in devices.keys(){publish(*id,&desired,&devices);}}
            }
        }
    }
    tasks.abort_all();
    archive_tasks.abort_all();
    while archive_tasks.join_next().await.is_some() {}
    probes.abort_all();
    while probes.join_next().await.is_some() {}
    while tasks.join_next().await.is_some() {}
    // Todos os futuros/túneis já foram destruídos na thread proprietária do SDK.
    drop(devices);
    drop(relay);
}
struct Context<'a> {
    device: &'a Device,
    password: &'a str,
    events: &'a Sender<Event>,
    frames: Frames,
    relay: Option<&'a Relay>,
}
impl Context<'_> {
    fn video(&self, slot: usize, request: VideoRequest, state: VideoState, message: String) {
        let _ = self.events.send(Event::Video {
            slot,
            generation: request.generation,
            state,
            message,
            details: None,
        });
    }
    fn playing(&self, slot: usize, request: VideoRequest, message: String, details: String) {
        let _ = self.events.send(Event::Video {
            slot,
            generation: request.generation,
            state: VideoState::Playing,
            message,
            details: Some(details),
        });
    }
    fn clear(&self, slot: usize) {
        if let Ok(mut frame) = self.frames[slot].lock() {
            *frame = None;
        }
    }
}
fn transient(error: &(dyn Error + 'static)) -> bool {
    if let Some(e) = error.downcast_ref::<sim_proto_dahua::Error>() {
        return matches!(
            e,
            sim_proto_dahua::Error::Io(_)
                | sim_proto_dahua::Error::Timeout
                | sim_proto_dahua::Error::Closed
        );
    }
    if let Some(e) = error.downcast_ref::<sim_proto_dvrip::Error>() {
        return matches!(
            e,
            sim_proto_dvrip::Error::Io(_)
                | sim_proto_dvrip::Error::Timeout
                | sim_proto_dvrip::Error::Closed
        );
    }
    if let Some(e) = error.downcast_ref::<sim_p2p_t2u::Error>() {
        return matches!(
            e,
            sim_p2p_t2u::Error::Timeout
                | sim_p2p_t2u::Error::Status(_)
                | sim_p2p_t2u::Error::Open(_)
        );
    }
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() != std::io::ErrorKind::InvalidInput)
}
async fn device_loop(
    device: Device,
    db: PathBuf,
    relay: Rc<RelayPool>,
    events: Sender<Event>,
    frames: Frames,
    mut requests: watch::Receiver<Requests>,
) -> Result<(), Failure> {
    let password = Store::open(&db)?.password(device.id)?;
    let ctx = Context {
        device: &device,
        password: &password,
        events: &events,
        frames,
        relay: None,
    };
    let mut failures = 0;
    loop {
        let state = if failures == 0 {
            DeviceState::Connecting
        } else {
            DeviceState::Reconnecting
        };
        let _ = events.send(Event::Device {
            id: device.id,
            state,
            message: if failures == 0 {
                "Conectando ao dispositivo…".into()
            } else {
                "Reconectando ao DVR…".into()
            },
        });
        let result = connect(&ctx, &mut requests, &relay).await;
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                let retry = transient(error.as_ref());
                for (slot, request) in requests.borrow().iter().enumerate() {
                    if let Some(request) = request {
                        ctx.clear(slot);
                        ctx.video(
                            slot,
                            *request,
                            if retry {
                                VideoState::Reconnecting
                            } else {
                                VideoState::Error
                            },
                            format!("Sem sinal: {error}"),
                        );
                    }
                }
                if !retry {
                    return Err(error);
                }
                failures += 1;
                tokio::time::sleep(retry_delay(failures)).await;
            }
        }
    }
}
async fn connect(
    ctx: &Context<'_>,
    requests: &mut watch::Receiver<Requests>,
    relay: &RelayPool,
) -> Result<(), Failure> {
    if ctx.device.config.connection == ConnectionMode::P2p {
        return connect_serial(ctx, &ctx.device.config.address, requests, relay).await;
    }
    let result = connect_address(ctx, requests).await;
    let Some(serial) = ctx.device.config.serial.as_deref() else {
        return result;
    };
    if result.is_ok() {
        return result;
    }
    ctx.events.send(Event::Device {
        id: ctx.device.id,
        state: DeviceState::Connecting,
        message: "Conexão por IP falhou; tentando pelo serial…".into(),
    })?;
    for (slot, request) in requests.borrow().iter().enumerate() {
        if let Some(request) = request {
            ctx.clear(slot);
            ctx.video(
                slot,
                *request,
                VideoState::Connecting,
                "IP indisponível; conectando pelo serial…".into(),
            );
        }
    }
    connect_serial(ctx, serial, requests, relay).await
}
async fn connect_address(
    ctx: &Context<'_>,
    requests: &mut watch::Receiver<Requests>,
) -> Result<(), Failure> {
    let endpoint = resolve_address(ctx.device).await?;
    serve(ctx, endpoint, requests).await
}
async fn resolve_address(device: &Device) -> Result<std::net::SocketAddr, Failure> {
    Ok(tokio::time::timeout(
        Duration::from_secs(8),
        tokio::net::lookup_host((device.config.address.as_str(), device.config.port)),
    )
    .await
    .map_err(|_| sim_proto_dahua::Error::Timeout)??
    .next()
    .ok_or("Endereço do dispositivo não encontrado.")?)
}

async fn probe(
    mut draft: sim_storage_import::NewDevice,
    existing: Option<i64>,
    db: &std::path::Path,
    pool: &RelayPool,
) -> Result<(Vec<Channel>, String), Failure> {
    draft.config.validate()?;
    if draft.password.is_empty()
        && let Some(id) = existing
    {
        draft.password = Store::open(db)?.password(id)?;
    }
    let device = Device {
        id: existing.unwrap_or(0),
        config: draft.config,
    };
    let (events, _) = std::sync::mpsc::channel();
    let ctx = Context {
        device: &device,
        password: &draft.password,
        events: &events,
        frames: std::array::from_fn(|_| Default::default()),
        relay: None,
    };
    let mut primary_failed = false;
    if device.config.connection == ConnectionMode::Address {
        let result = async {
            let endpoint = resolve_address(&device).await?;
            let mut session = Session::open(&ctx, endpoint).await?;
            session.channels().await
        }
        .await;
        match result {
            Ok(channels) => return probe_result(channels, "IP / domínio"),
            Err(error) => {
                if device.config.serial.is_none() {
                    return Err(error);
                }
                primary_failed = true;
            }
        }
    }
    let serial = device
        .config
        .serial
        .as_deref()
        .unwrap_or(&device.config.address);
    let relay = pool.get()?;
    let tunnel = relay.tunnel(serial, device.config.port).await?;
    let mut session = Session::open(&ctx, tunnel.endpoint()).await?;
    let channels = session.channels().await?;
    probe_result(
        channels,
        if primary_failed {
            "serial, após falha no IP"
        } else {
            "serial"
        },
    )
}
fn probe_result(channels: Vec<Channel>, route: &str) -> Result<(Vec<Channel>, String), Failure> {
    if channels.is_empty() {
        return Err("O DVR não informou canais de vídeo.".into());
    }
    let message = format!(
        "Conexão testada por {route}: {} canais encontrados.",
        channels.len()
    );
    Ok((channels, message))
}
async fn connect_serial(
    ctx: &Context<'_>,
    serial: &str,
    requests: &mut watch::Receiver<Requests>,
    pool: &RelayPool,
) -> Result<(), Failure> {
    let relay = pool.get()?;
    let mut device = ctx.device.clone();
    device.config.address = serial.into();
    device.config.connection = ConnectionMode::P2p;
    device.config.serial = None;
    let selected = Context {
        device: &device,
        password: ctx.password,
        events: ctx.events,
        frames: ctx.frames.clone(),
        relay: Some(&relay),
    };
    let tunnel = relay.tunnel(serial, device.config.port).await?;
    serve(&selected, tunnel.endpoint(), requests).await
}

enum Session {
    Dahua(sim_proto_dahua::Session),
    Jufeng(sim_proto_dvrip::Session),
}
impl Session {
    async fn open(
        ctx: &Context<'_>,
        endpoint: std::net::SocketAddr,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(match ctx.device.config.manufacturer {
            Manufacturer::Dahua => Self::Dahua(
                sim_proto_dahua::Session::connect(
                    endpoint,
                    &ctx.device.config.username,
                    ctx.password,
                    sim_proto_dahua::WireMode::Legacy,
                )
                .await?,
            ),
            Manufacturer::Jufeng => Self::Jufeng(
                sim_proto_dvrip::Session::connect(
                    endpoint,
                    &ctx.device.config.username,
                    ctx.password,
                )
                .await?,
            ),
        })
    }
    async fn channels(&mut self) -> Result<Vec<Channel>, Box<dyn Error>> {
        Ok(match self {
            Self::Dahua(s) => s.channels().await?,
            Self::Jufeng(s) => s.channels().await?,
        })
    }
    async fn keep_alive(&mut self) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Dahua(s) => s.keep_alive().await?,
            Self::Jufeng(s) => s.keep_alive().await?,
        };
        Ok(())
    }
    fn interval(&self) -> Duration {
        match self {
            Self::Dahua(s) => s.keep_alive_interval,
            Self::Jufeng(s) => s.keep_alive_interval,
        }
    }
}
async fn serve(
    ctx: &Context<'_>,
    endpoint: std::net::SocketAddr,
    requests: &mut watch::Receiver<Requests>,
) -> Result<(), Failure> {
    let mut session = Session::open(ctx, endpoint).await?;
    let channels = session.channels().await?;
    let available: Vec<u16> = channels.iter().map(|c| c.number).collect();
    session.keep_alive().await?;
    ctx.events.send(Event::Online {
        id: ctx.device.id,
        channels,
    })?;
    ctx.events.send(Event::Device {
        id: ctx.device.id,
        state: DeviceState::Online,
        message: if ctx.device.config.connection == ConnectionMode::P2p {
            "Conectado pelo serial (Cloud/P2P)."
        } else {
            "Conectado por IP / domínio."
        }
        .into(),
    })?;
    let mut keep = tokio::time::interval(session.interval() / 2);
    keep.tick().await;
    let video = players::run(ctx, endpoint, &available, requests);
    tokio::pin!(video);
    loop {
        tokio::select! {_=keep.tick()=>session.keep_alive().await?,result=&mut video=>return result}
    }
}
