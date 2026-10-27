use crate::{
    AppWindow, VideoTile,
    mosaic::{Binding, CAPACITY, QualityMode, Slot, first_channels},
    network::{self, DeviceState, VideoRequest, VideoState},
};
use sim_core::Device;
use slint::{Model, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::mpsc::Sender,
};

pub struct Controller {
    network: network::Handle,
    frames: network::Frames,
    slots: RefCell<[Slot; CAPACITY]>,
    rows: Rc<VecModel<VideoTile>>,
    states: RefCell<HashMap<i64, (DeviceState, String)>>,
    channels: RefCell<HashMap<i64, Vec<sim_core::Channel>>>,
    pending_bulk: RefCell<Option<(Device, i32)>>,
    gpu: Cell<bool>,
    quality: Cell<QualityMode>,
    pub displayed: Cell<[u64; CAPACITY]>,
}
impl Controller {
    pub fn archive(&self, token: u64, device: Device, job: network::archive::Job) -> bool {
        self.network.archive(token, device, job)
    }
    pub fn cancel_archive(&self, export: bool) {
        self.network.cancel_archive(export);
    }
    pub fn new(
        ui: &AppWindow,
        db: PathBuf,
        settings: network::Settings,
        events: Sender<network::Event>,
    ) -> Self {
        let frames = std::array::from_fn(|_| Default::default());
        let rows = Rc::new(VecModel::from(vec![VideoTile::default(); CAPACITY]));
        ui.set_video_tiles(ModelRc::from(rows.clone()));
        crate::layouts::initialize(ui);
        crate::layouts::apply(ui, 1);
        Self {
            network: network::start(db, settings, events, frames.clone()),
            frames,
            slots: RefCell::new(std::array::from_fn(|_| Slot::default())),
            rows,
            states: RefCell::new(HashMap::new()),
            channels: RefCell::new(HashMap::new()),
            pending_bulk: RefCell::new(None),
            gpu: Cell::new(false),
            quality: Cell::new(QualityMode::Sd),
            displayed: Cell::new([0; CAPACITY]),
        }
    }
    pub fn state(&self, id: i64) -> DeviceState {
        self.states
            .borrow()
            .get(&id)
            .map(|s| s.0)
            .unwrap_or(DeviceState::Offline)
    }
    pub fn probe(
        &self,
        token: u64,
        draft: sim_storage_import::NewDevice,
        existing: Option<i64>,
    ) -> bool {
        self.network.probe(token, draft, existing)
    }
    pub fn cancel_probe(&self, token: u64) {
        self.network.cancel_probe(token);
    }
    pub fn set_gpu(&self, ui: &AppWindow, enabled: bool) {
        if self.gpu.replace(enabled) == enabled {
            return;
        }
        for slot in 0..CAPACITY {
            if self.slots.borrow()[slot].binding.is_some() {
                self.slots.borrow_mut()[slot].invalidate();
                self.queue(ui, slot);
            }
        }
    }
    pub fn snapshot(&self, ui: &AppWindow, devices: &[Device]) -> crate::preferences::View {
        use crate::preferences::{SavedSlot, View};
        View {
            layout: ui.get_layout_size(),
            layout_preset: Some(ui.get_layout_index() as usize),
            selected: ui.get_selected_slot(),
            expanded: ui.get_tile_expanded(),
            quality: Some(self.quality.get().index()),
            slots: self
                .slots
                .borrow()
                .iter()
                .map(|slot| {
                    let b = slot.binding.as_ref()?;
                    let d = devices.iter().find(|d| d.id == b.device)?;
                    Some(SavedSlot {
                        device: b.device,
                        address: d.config.address.clone(),
                        channel: b.channel,
                        quality: slot.mode.index(),
                    })
                })
                .collect(),
        }
    }
    pub fn restore(&self, ui: &AppWindow, devices: &[Device], view: &crate::preferences::View) {
        self.quality(
            ui,
            if view.quality() == QualityMode::Hd {
                1
            } else {
                0
            },
        );
        crate::layouts::apply(ui, view.preset());
        ui.set_selected_slot(view.selected());
        ui.set_tile_expanded(view.expanded);
        for (slot, saved) in view.slots.iter().take(CAPACITY).enumerate() {
            if let Some(saved) = saved
                && let Some(device) = saved.resolve(devices)
            {
                self.assign(ui, device, saved.channel, slot);
            }
        }
    }
    pub fn status(&self, slot: usize) -> String {
        self.rows
            .row_data(slot)
            .map(|r| r.status.to_string())
            .unwrap_or_default()
    }
    pub fn details(&self, slot: usize) -> String {
        self.rows
            .row_data(slot)
            .map(|r| r.details.to_string())
            .unwrap_or_default()
    }
    pub fn connect(&self, ui: &AppWindow, device: Device) {
        if self.state(device.id).active() {
            return;
        }
        let id = device.id;
        self.states.borrow_mut().insert(
            id,
            (DeviceState::Connecting, "Conectando ao dispositivo…".into()),
        );
        if !self.network.connect(device) {
            self.states.borrow_mut().insert(
                id,
                (DeviceState::Error, "Fila de comandos indisponível.".into()),
            );
        }
        self.decorate(ui);
    }
    pub fn disconnect(&self, ui: &AppWindow, id: i64) {
        let affected: Vec<usize> = self
            .slots
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.binding.as_ref().filter(|b| b.device == id).map(|_| i))
            .collect();
        for slot in affected {
            self.close(ui, slot);
        }
        if !self.network.disconnect(id) {
            ui.set_status("Não foi possível agendar a desconexão.".into());
            return;
        }
        self.states.borrow_mut().insert(
            id,
            (DeviceState::Offline, "Dispositivo desconectado.".into()),
        );
        self.decorate(ui);
    }
    pub fn open_device(&self, ui: &AppWindow, device: &Device) {
        // O clique mais recente substitui uma abertura em lote ainda pendente.
        self.pending_bulk.borrow_mut().take();
        ui.set_tile_expanded(false);
        ui.set_selected_slot(0);
        if self.state(device.id) == DeviceState::Online {
            let channels = self
                .channels
                .borrow()
                .get(&device.id)
                .cloned()
                .unwrap_or_else(|| device.config.channels.clone());
            self.fill_layout(ui, device, &channels, ui.get_layout_size());
        } else {
            *self.pending_bulk.borrow_mut() = Some((device.clone(), ui.get_layout_size()));
            self.connect(ui, device.clone());
        }
    }
    fn fill_layout(
        &self,
        ui: &AppWindow,
        device: &Device,
        channels: &[sim_core::Channel],
        limit: i32,
    ) {
        let limit = limit.min(ui.get_layout_size());
        let channels = first_channels(channels, limit);
        let mut device = device.clone();
        device.config.channels = channels.clone();
        for slot in 0..ui.get_layout_size().clamp(1, CAPACITY as i32) as usize {
            if let Some(channel) = channels.get(slot) {
                self.assign(ui, &device, channel.number, slot);
            } else if self.slots.borrow()[slot].binding.is_some() {
                self.close(ui, slot);
            }
        }
    }
    pub fn assign(&self, ui: &AppWindow, device: &Device, channel: u16, slot: usize) {
        if slot >= CAPACITY {
            return;
        }
        let Some(channel) = device.config.channels.iter().find(|c| c.number == channel) else {
            ui.set_status("Canal não encontrado no cadastro.".into());
            return;
        };
        self.pending_bulk.borrow_mut().take();
        self.connect(ui, device.clone());
        self.slots.borrow_mut()[slot].assign(
            Binding {
                device: device.id,
                channel: channel.number,
                title: format!("{} · {}", device.config.name, channel.name),
            },
            self.quality.get(),
        );
        self.queue(ui, slot);
    }
    fn queue(&self, ui: &AppWindow, slot: usize) {
        let slots = self.slots.borrow();
        let s = &slots[slot];
        let Some(binding) = &s.binding else {
            return;
        };
        let sent = self.network.play(
            slot,
            binding.device,
            VideoRequest {
                channel: binding.channel,
                generation: s.generation,
                quality: s.quality,
                fallback: false,
                gpu: self.gpu.get(),
            },
        );
        self.rows.set_row_data(
            slot,
            VideoTile {
                title: binding.title.clone().into(),
                status: if sent {
                    "Conectando…"
                } else {
                    "Fila de comandos indisponível."
                }
                .into(),
                assigned: true,
                ..Default::default()
            },
        );
        let mut counts = self.displayed.get();
        counts[slot] = 0;
        self.displayed.set(counts);
        drop(slots);
        self.update_count(ui);
    }
    pub fn close(&self, ui: &AppWindow, slot: usize) {
        if slot >= CAPACITY {
            return;
        }
        self.pending_bulk.borrow_mut().take();
        if !self.network.stop_video(slot) {
            ui.set_status("Não foi possível agendar o fechamento do canal.".into());
            return;
        }
        self.slots.borrow_mut()[slot].clear();
        self.rows.set_row_data(slot, VideoTile::default());
        if let Ok(mut frame) = self.frames[slot].lock() {
            *frame = None;
        }
        self.update_count(ui);
    }
    pub fn stop_all(&self, ui: &AppWindow) {
        self.pending_bulk.borrow_mut().take();
        if !self.network.stop_all() {
            ui.set_status("Não foi possível agendar a parada dos vídeos.".into());
            return;
        }
        for slot in 0..CAPACITY {
            self.slots.borrow_mut()[slot].clear();
            self.rows.set_row_data(slot, VideoTile::default());
            if let Ok(mut f) = self.frames[slot].lock() {
                *f = None;
            }
        }
        self.update_count(ui);
    }
    pub fn quality(&self, ui: &AppWindow, index: i32) {
        let mode = if index == 1 {
            QualityMode::Hd
        } else {
            QualityMode::Sd
        };
        self.quality.set(mode);
        ui.set_global_quality(if mode == QualityMode::Hd { 1 } else { 0 });
        for slot in 0..CAPACITY {
            let changed = self.slots.borrow_mut()[slot].set_quality(mode);
            if changed {
                self.queue(ui, slot);
            }
        }
    }
    pub fn view_changed(&self, ui: &AppWindow) {
        // Changing geometry preserves every assigned camera, including hidden positions.
        let preset =
            crate::layouts::index(Some(ui.get_layout_index() as usize), ui.get_layout_size());
        crate::layouts::apply(ui, preset);
    }
    pub fn event(
        &self,
        ui: &AppWindow,
        event: network::Event,
    ) -> Option<(i64, Vec<sim_core::Channel>)> {
        match event {
            network::Event::Archive { .. } => {}
            network::Event::Probe { .. } => {}
            network::Event::Online { id, channels } => {
                self.channels.borrow_mut().insert(id, channels.clone());
                let pending = {
                    let mut pending = self.pending_bulk.borrow_mut();
                    if pending.as_ref().is_some_and(|(device, _)| device.id == id) {
                        pending.take()
                    } else {
                        None
                    }
                };
                if let Some((device, limit)) = pending {
                    self.fill_layout(ui, &device, &channels, limit);
                }
                return Some((id, channels));
            }
            network::Event::Device { id, state, message } => {
                if matches!(state, DeviceState::Error | DeviceState::Offline)
                    && self
                        .pending_bulk
                        .borrow()
                        .as_ref()
                        .is_some_and(|(device, _)| device.id == id)
                {
                    self.pending_bulk.borrow_mut().take();
                }
                if !matches!(state, DeviceState::Online | DeviceState::Connecting) {
                    for (slot, s) in self.slots.borrow().iter().enumerate() {
                        if s.binding.as_ref().is_some_and(|b| b.device == id)
                            && let Some(mut row) = self.rows.row_data(slot)
                        {
                            row.playing = false;
                            row.image = Default::default();
                            row.details = Default::default();
                            row.status = message.clone().into();
                            self.rows.set_row_data(slot, row);
                        }
                    }
                }
                self.states.borrow_mut().insert(id, (state, message));
                self.decorate(ui);
            }
            network::Event::Video {
                slot,
                generation,
                state,
                message,
                details,
            } => {
                if slot < CAPACITY
                    && self.slots.borrow()[slot].accepts(generation)
                    && let Some(mut row) = self.rows.row_data(slot)
                {
                    if state != VideoState::Playing {
                        row.playing = false;
                        row.image = Default::default();
                    }
                    row.details = details.unwrap_or_default().into();
                    row.status = message.into();
                    self.rows.set_row_data(slot, row);
                }
            }
        }
        None
    }
    pub fn poll(&self) {
        let mut counts = self.displayed.get();
        for (slot, frames) in self.frames.iter().enumerate() {
            if let Some(frame) = frames.lock().ok().and_then(|mut f| f.take())
                && self.slots.borrow()[slot].accepts(frame.generation)
                && let Some(mut row) = self.rows.row_data(slot)
            {
                let pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(
                    &frame.rgb,
                    frame.width,
                    frame.height,
                );
                row.image = slint::Image::from_rgb8(pixels);
                row.playing = true;
                self.rows.set_row_data(slot, row);
                counts[slot] += 1;
            }
        }
        self.displayed.set(counts);
    }
    fn update_count(&self, ui: &AppWindow) {
        ui.set_active_videos(
            self.slots
                .borrow()
                .iter()
                .filter(|s| s.binding.is_some())
                .count() as i32,
        );
    }
    pub fn decorate(&self, ui: &AppWindow) {
        let states = self.states.borrow();
        let rows: Vec<_> = ui
            .get_devices()
            .iter()
            .map(|mut row| {
                let state = row
                    .id
                    .as_str()
                    .parse::<i64>()
                    .ok()
                    .and_then(|id| states.get(&id));
                row.state = match state.map(|s| s.0).unwrap_or(DeviceState::Offline) {
                    DeviceState::Offline => 0,
                    DeviceState::Connecting => 1,
                    DeviceState::Online => 2,
                    DeviceState::Reconnecting => 3,
                    DeviceState::Error => 4,
                };
                row.state_text = match row.state {
                    1 => "Conectando…",
                    2 => "Conectado",
                    3 => "Reconectando…",
                    4 => "Falha na conexão",
                    _ => "Desconectado",
                }
                .into();
                row
            })
            .collect();
        ui.set_devices(ModelRc::new(VecModel::from(rows)));
        drop(states);
        self.selected_status(ui);
    }
    pub fn selected_status(&self, ui: &AppWindow) {
        let states = self.states.borrow();
        let state = ui
            .get_selected_id()
            .as_str()
            .parse::<i64>()
            .ok()
            .and_then(|id| states.get(&id));
        ui.set_network_active(state.is_some_and(|s| s.0.active()));
        ui.set_network_online(state.is_some_and(|s| s.0 == DeviceState::Online));
        ui.set_connection_message(
            state
                .map(|s| s.1.clone())
                .unwrap_or_else(|| {
                    "Clique numa câmera para abrir ou no nome do DVR para preencher o layout."
                        .into()
                })
                .into(),
        );
    }
}
