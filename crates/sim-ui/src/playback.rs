use crate::{
    AppWindow, PlaybackChannel, TimelineSegment, VideoTile,
    network::{
        self,
        archive::{Job, SLOTS, Update},
    },
    video,
};
use sim_core::{
    Device,
    recording::{self, Recording, Stamp},
};
use slint::{ComponentHandle, Model, VecModel};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, atomic::Ordering},
};

#[derive(Clone)]
struct Search {
    device: Device,
    channels: Vec<u16>,
    start: Stamp,
    end: Stamp,
}
pub struct Controller {
    video: Rc<video::Controller>,
    devices: RefCell<Vec<Device>>,
    channels: Rc<VecModel<PlaybackChannel>>,
    tiles: Rc<VecModel<VideoTile>>,
    search: RefCell<Option<Search>>,
    records: RefCell<Vec<Recording>>,
    frames: network::archive::ArchiveFrames,
    controls: RefCell<[Arc<sim_media::archive::PlaybackControl>; SLOTS]>,
    sequence: Cell<u64>,
    token: Cell<u64>,
    export_token: Cell<u64>,
    start_second: Cell<u32>,
    pub displayed: Cell<u64>,
}
impl Controller {
    pub fn new(ui: &AppWindow, video: Rc<video::Controller>) -> Rc<Self> {
        let channels = Rc::new(VecModel::default());
        let tiles = Rc::new(VecModel::from(vec![VideoTile::default(); SLOTS]));
        ui.set_playback_channels(channels.clone().into());
        ui.set_playback_tiles(tiles.clone().into());
        let mut now = unsafe { std::mem::zeroed() };
        unsafe {
            windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut now);
        }
        ui.set_playback_date(format!("{:04}-{:02}-{:02}", now.wYear, now.wMonth, now.wDay).into());
        Self::choose_date(ui, now.wYear as i32, now.wMonth as i32, now.wDay as i32);
        Rc::new(Self {
            video,
            devices: Default::default(),
            channels,
            tiles,
            search: Default::default(),
            records: Default::default(),
            frames: std::array::from_fn(|_| Default::default()),
            controls: RefCell::new(std::array::from_fn(|_| Default::default())),
            sequence: Cell::new(0),
            token: Cell::new(0),
            export_token: Cell::new(0),
            start_second: Cell::new(0),
            displayed: Cell::new(0),
        })
    }
    fn next(&self) -> u64 {
        let token = self.sequence.get() + 1;
        self.sequence.set(token);
        token
    }
    fn choose_date(ui: &AppWindow, year: i32, month: i32, day: i32) {
        let date = format!("{year:04}-{month:02}-{day:02}");
        if Stamp::parse(&format!("{date} 00:00:00")).is_err() {
            ui.set_playback_message("Selecione uma data válida entre 2000 e 2099.".into());
            return;
        }
        ui.set_playback_date(date.into());
        ui.set_playback_date_label(format!("{day:02}/{month:02}/{year:04}").into());
        ui.set_playback_calendar_date(crate::Date { year, month, day });
    }
    fn calendar_month(ui: &AppWindow, year: i32, month: i32) {
        let date = ui.get_playback_calendar_date();
        let mut today = unsafe { std::mem::zeroed() };
        unsafe {
            windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut today);
        }
        let cells = crate::calendar::cells(
            year,
            month,
            (date.year, date.month, date.day),
            (today.wYear as i32, today.wMonth as i32, today.wDay as i32),
        );
        ui.set_calendar_year(year);
        ui.set_calendar_month(month);
        ui.set_calendar_days(Rc::new(VecModel::from(cells)).into());
    }
    pub fn refresh(&self, ui: &AppWindow, devices: &[Device]) {
        *self.devices.borrow_mut() = devices.to_vec();
        ui.set_playback_devices(
            Rc::new(VecModel::from(
                devices
                    .iter()
                    .map(|d| d.config.name.clone().into())
                    .collect::<Vec<slint::SharedString>>(),
            ))
            .into(),
        );
        if self.channels.row_count() == 0 && !devices.is_empty() {
            ui.set_playback_device(0);
            self.select(ui, 0);
        }
    }
    pub fn select(&self, ui: &AppWindow, index: i32) {
        if let Some(device) = self.devices.borrow().get(index.max(0) as usize) {
            self.channels.set_vec(
                device
                    .config
                    .channels
                    .iter()
                    .map(|c| PlaybackChannel {
                        number: c.number as i32,
                        name: c.name.clone().into(),
                        checked: false,
                    })
                    .collect::<Vec<_>>(),
            );
            ui.set_playback_message("Selecione uma câmera e o período da busca.".into());
        }
    }
    pub fn toggle(&self, _ui: &AppWindow, number: i32, checked: bool) {
        if !self.channels.iter().any(|row| row.number == number) {
            return;
        }
        for i in 0..self.channels.row_count() {
            let mut row = self.channels.row_data(i).unwrap();
            if row.number == number {
                row.checked = checked;
            } else if checked {
                row.checked = false;
            }
            self.channels.set_row_data(i, row);
        }
    }
    pub fn search(&self, ui: &AppWindow) {
        let result = (|| -> Result<Search, &'static str> {
            let device = self
                .devices
                .borrow()
                .get(ui.get_playback_device().max(0) as usize)
                .cloned()
                .ok_or("Cadastre e selecione um DVR.")?;
            let channels: Vec<_> = self
                .channels
                .iter()
                .filter(|c| c.checked)
                .map(|c| c.number as u16)
                .collect();
            if channels.len() != SLOTS {
                return Err("Selecione uma câmera para buscar gravações.");
            }
            let (start, end) = recording::period(
                &format!(
                    "{} {}",
                    ui.get_playback_date().trim(),
                    ui.get_playback_from().trim()
                ),
                &format!(
                    "{} {}",
                    ui.get_playback_date().trim(),
                    ui.get_playback_to().trim()
                ),
            )?;
            Ok(Search {
                device,
                channels,
                start,
                end,
            })
        })();
        let search = match result {
            Ok(s) => s,
            Err(e) => {
                ui.set_playback_message(e.into());
                return;
            }
        };
        self.stop(ui);
        ui.set_playback_selected(0);
        ui.set_playback_cursor(0.0);
        self.records.borrow_mut().clear();
        ui.set_playback_ready(false);
        ui.set_playback_segments(Default::default());
        let token = self.next();
        self.token.set(token);
        if !self.video.archive(
            token,
            search.device.clone(),
            Job::Search {
                channels: search.channels.clone(),
                start: search.start.clone(),
                end: search.end.clone(),
            },
        ) {
            ui.set_playback_message("Não foi possível iniciar a busca.".into());
            return;
        }
        *self.search.borrow_mut() = Some(search);
        ui.set_playback_busy(true);
        ui.set_playback_message("Buscando gravações no DVR…".into());
    }
    pub fn event(&self, ui: &AppWindow, token: u64, update: Update) {
        if token == self.export_token.get() {
            match update {
                Update::ExportProgress(percent) => {
                    ui.set_export_message(format!("Exportando MP4… {percent}%").into())
                }
                Update::Exported(path) => {
                    ui.set_export_busy(false);
                    ui.set_export_message(format!("MP4 salvo: {}", path.display()).into());
                }
                Update::Error(e) => {
                    ui.set_export_busy(false);
                    ui.set_export_message(e.into());
                }
                _ => {}
            }
            return;
        }
        if token != self.token.get() {
            return;
        }
        match update {
            Update::SlotError { slot, message } => {
                if let Some(mut row) = self.tiles.row_data(slot) {
                    row.playing = false;
                    row.status = message.into();
                    self.tiles.set_row_data(slot, row);
                }
            }
            Update::Found(records) => {
                ui.set_playback_busy(false);
                let Some(search) = self.search.borrow().clone() else {
                    return;
                };
                let span = (search.end.second() - search.start.second()) as f32;
                let segments: Vec<_> = records
                    .iter()
                    .filter_map(|r| {
                        let slot = search.channels.iter().position(|&c| c == r.channel)?;
                        let start = if r.start < search.start {
                            search.start.second()
                        } else {
                            r.start.second()
                        };
                        let end = if r.end > search.end {
                            search.end.second()
                        } else {
                            r.end.second()
                        };
                        Some(TimelineSegment {
                            slot: slot as i32,
                            start: (start - search.start.second()) as f32 / span,
                            end: (end - search.start.second()) as f32 / span,
                            kind: if r.kind.contains("Motion") {
                                2
                            } else if r.kind.contains("Alarm") {
                                1
                            } else {
                                0
                            },
                        })
                    })
                    .collect();
                ui.set_playback_segments(Rc::new(VecModel::from(segments)).into());
                let labels = search
                    .channels
                    .iter()
                    .map(|number| {
                        search
                            .device
                            .config
                            .channels
                            .iter()
                            .find(|c| c.number == *number)
                            .map(|c| c.name.clone().into())
                            .unwrap_or_else(|| format!("Canal {}", number + 1).into())
                    })
                    .collect::<Vec<slint::SharedString>>();
                ui.set_playback_names(Rc::new(VecModel::from(labels)).into());
                ui.set_playback_range_start(search.start.0[11..].into());
                ui.set_playback_range_end(search.end.0[11..].into());
                ui.set_playback_ready(!records.is_empty());
                ui.set_playback_message(if records.is_empty() {
                    "Nenhuma gravação neste período.".into()
                } else {
                    format!(
                        "{} intervalos encontrados. Abrindo gravação…",
                        records.len()
                    )
                    .into()
                });
                let available = !records.is_empty();
                *self.records.borrow_mut() = records;
                if available {
                    self.seek(ui, 0.0);
                }
            }
            Update::Playing => {
                ui.set_playback_busy(false);
                ui.set_playback_running(true);
                ui.set_playback_message(
                    format!(
                        "Reprodução · {}×",
                        1u32 << ui.get_playback_speed_index().clamp(0, 4)
                    )
                    .into(),
                );
            }
            Update::Finished => {
                ui.set_playback_running(false);
                ui.set_playback_message("Fim do período de reprodução.".into());
            }
            Update::Error(e) => {
                ui.set_playback_busy(false);
                ui.set_playback_running(false);
                ui.set_playback_message(e.into());
            }
            _ => {}
        }
    }
    pub fn seek(&self, ui: &AppWindow, fraction: f32) {
        let Some(search) = self.search.borrow().clone() else {
            return;
        };
        let mut second = search.start.second()
            + ((search.end.second() - search.start.second()) as f32 * fraction.clamp(0.0, 0.99999))
                as u32;
        let records = self.records.borrow();
        if !records.iter().any(|r| {
            r.start <= search.start.at_second(second) && r.end > search.start.at_second(second)
        }) {
            if let Some(next) = records
                .iter()
                .filter(|r| r.start > search.start.at_second(second))
                .map(|r| r.start.second())
                .min()
            {
                second = next;
            } else {
                ui.set_playback_message("Não há gravação a partir deste horário.".into());
                return;
            }
        }
        drop(records);
        self.stop(ui);
        self.start_second.set(second);
        let token = self.next();
        self.token.set(token);
        *self.controls.borrow_mut() = std::array::from_fn(|_| Default::default());
        for control in self.controls.borrow().iter() {
            control.set_speed(1u32 << ui.get_playback_speed_index().clamp(0, 4));
            control.buffering.store(true, Ordering::Release);
        }
        for i in 0..SLOTS {
            if let Ok(mut frame) = self.frames[i].lock() {
                *frame = None;
            }
            let title = search
                .channels
                .get(i)
                .map(|number| {
                    search
                        .device
                        .config
                        .channels
                        .iter()
                        .find(|c| c.number == *number)
                        .map(|c| format!("{} · {}", search.device.config.name, c.name))
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            self.tiles.set_row_data(
                i,
                VideoTile {
                    title: title.into(),
                    assigned: i < search.channels.len(),
                    status: "Abrindo gravação…".into(),
                    ..Default::default()
                },
            );
        }
        let started = self.video.archive(
            token,
            search.device.clone(),
            Job::Play {
                channels: search.channels,
                start: search.start.at_second(second),
                end: search.end,
                frames: self.frames.clone(),
                controls: self.controls.borrow().clone(),
                gpu: ui.get_option_gpu(),
            },
        );
        ui.set_playback_busy(started);
        ui.set_playback_paused(false);
        ui.set_playback_message(
            if started {
                "Conectando à gravação…"
            } else {
                "Não foi possível iniciar a reprodução."
            }
            .into(),
        );
        self.cursor(ui, second);
    }
    pub fn play_pause(&self, ui: &AppWindow) {
        if !ui.get_playback_running() {
            self.seek(ui, ui.get_playback_cursor());
            return;
        }
        let paused = !ui.get_playback_paused();
        for control in self.controls.borrow().iter() {
            control.paused.store(paused, Ordering::Release);
        }
        ui.set_playback_paused(paused);
    }
    pub fn speed(&self, ui: &AppWindow, index: i32) {
        if !(0..=4).contains(&index) {
            return;
        }
        ui.set_playback_speed_index(index);
        if ui.get_playback_running() || (ui.get_playback_busy() && ui.get_playback_ready()) {
            let paused = ui.get_playback_paused();
            self.seek(ui, ui.get_playback_cursor());
            for control in self.controls.borrow().iter() {
                control.paused.store(paused, Ordering::Release);
            }
            ui.set_playback_paused(paused);
        }
    }
    pub fn stop(&self, ui: &AppWindow) {
        self.token.set(self.next());
        self.video.cancel_archive(false);
        ui.set_playback_running(false);
        ui.set_playback_busy(false);
        ui.set_playback_paused(false);
        ui.set_playback_buffering(false);
    }
    fn cursor(&self, ui: &AppWindow, second: u32) {
        if let Some(search) = self.search.borrow().as_ref() {
            ui.set_playback_cursor(
                (second.saturating_sub(search.start.second())) as f32
                    / (search.end.second() - search.start.second()) as f32,
            );
            ui.set_playback_clock(search.start.at_second(second).0[11..].into());
        }
    }
    pub fn poll(&self, ui: &AppWindow) {
        ui.set_playback_buffering(
            ui.get_playback_running()
                && self
                    .controls
                    .borrow()
                    .iter()
                    .any(|control| control.buffering.load(Ordering::Acquire)),
        );
        for (slot, frames) in self.frames.iter().enumerate() {
            if let Some(frame) = frames.lock().ok().and_then(|mut f| f.take())
                && frame.generation == self.token.get()
            {
                let mut row = self.tiles.row_data(slot).unwrap();
                row.image = slint::Image::from_rgb8(
                    slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(
                        &frame.rgb,
                        frame.width,
                        frame.height,
                    ),
                );
                row.playing = true;
                self.tiles.set_row_data(slot, row);
                self.displayed.set(self.displayed.get() + 1);
            }
        }
        if ui.get_playback_running() {
            let millis = self
                .controls
                .borrow()
                .iter()
                .map(|c| c.position_ms.load(Ordering::Acquire))
                .max()
                .unwrap_or(0);
            self.cursor(ui, self.start_second.get() + (millis / 1000) as u32);
        }
    }
    pub fn prepare_export(&self, ui: &AppWindow) {
        let Some(search) = self.search.borrow().clone() else {
            return;
        };
        let start = self
            .start_second
            .get()
            .max(search.start.second())
            .min(search.end.second() - 1);
        ui.set_export_channel(
            ui.get_playback_names()
                .row_data(ui.get_playback_selected().max(0) as usize)
                .unwrap_or_default(),
        );
        ui.set_export_from(search.start.at_second(start).0[11..].into());
        ui.set_export_to(
            search
                .start
                .at_second((start + 300).min(search.end.second()))
                .0[11..]
                .into(),
        );
        ui.set_export_quality(0);
        self.refresh_export_name(ui, true);
        ui.set_export_message("HD: principal. SD: extra gravado no DVR. O MP4 preserva a resolução original, sem áudio.".into());
        ui.set_export_dialog(true);
    }
    fn refresh_export_name(&self, ui: &AppWindow, reset: bool) {
        let search = self.search.borrow();
        let Some(search) = search.as_ref() else {
            return;
        };
        let Some(&channel) = search
            .channels
            .get(ui.get_playback_selected().max(0) as usize)
        else {
            return;
        };
        let Ok((start, end)) = recording::period(
            &format!("{} {}", &search.start.0[..10], ui.get_export_from().trim()),
            &format!("{} {}", &search.start.0[..10], ui.get_export_to().trim()),
        ) else {
            return;
        };
        let devices = self.devices.borrow();
        let device = devices
            .iter()
            .find(|d| d.id == search.device.id)
            .unwrap_or(&search.device);
        let channel = device
            .config
            .channels
            .iter()
            .find(|c| c.number == channel)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("Canal {}", channel + 1));
        let name = crate::export_name::filename(&device.config.name, &channel, &start, &end);
        let previous = ui.get_export_filename();
        let current = std::path::PathBuf::from(ui.get_export_path().as_str());
        if reset
            || current.as_os_str().is_empty()
            || current.file_name().is_some_and(|n| n == previous.as_str())
        {
            let path = if current.is_absolute() {
                current.parent().unwrap().join(&name)
            } else {
                std::path::PathBuf::from(&name)
            };
            ui.set_export_path(path.to_string_lossy().into_owned().into());
        }
        ui.set_export_filename(name.into());
    }
    pub fn export(&self, ui: &AppWindow) {
        self.refresh_export_name(ui, false);
        let Some(search) = self.search.borrow().clone() else {
            return;
        };
        let period = recording::period(
            &format!("{} {}", &search.start.0[..10], ui.get_export_from().trim()),
            &format!("{} {}", &search.start.0[..10], ui.get_export_to().trim()),
        );
        let (start, end) = match period {
            Ok(p) => p,
            Err(e) => {
                ui.set_export_message(e.into());
                return;
            }
        };
        if start < search.start || end > search.end {
            ui.set_export_message("Escolha um trecho dentro do período pesquisado.".into());
            return;
        }
        let Some(&channel) = search
            .channels
            .get(ui.get_playback_selected().max(0) as usize)
        else {
            ui.set_export_message("Selecione uma câmera do mosaico.".into());
            return;
        };
        let path = std::path::PathBuf::from(ui.get_export_path().as_str());
        if path
            .extension()
            .is_none_or(|e| !e.eq_ignore_ascii_case("mp4"))
            || !path.is_absolute()
        {
            ui.set_export_message("Escolha o arquivo MP4 de destino.".into());
            return;
        }
        let token = self.next();
        self.export_token.set(token);
        let quality = match ui.get_export_quality() {
            0 => sim_media::StreamQuality::Hd,
            1 => sim_media::StreamQuality::Sd,
            _ => {
                ui.set_export_message("Selecione HD ou SD.".into());
                return;
            }
        };
        if self.video.archive(
            token,
            search.device,
            Job::Export {
                channel,
                start,
                end,
                path,
                quality,
            },
        ) {
            ui.set_export_busy(true);
            ui.set_export_message(format!("Preparando exportação {}…", quality.label()).into());
        }
    }
    pub fn cancel_export(&self, ui: &AppWindow) {
        self.export_token.set(self.next());
        self.video.cancel_archive(true);
        ui.set_export_busy(false);
        ui.set_export_message("Exportação cancelada.".into());
    }
    pub fn wire(self: &Rc<Self>, ui: &AppWindow) {
        ui.on_open_calendar({
            let weak = ui.as_weak();
            move || {
                if let Some(ui) = weak.upgrade() {
                    let date = ui.get_playback_calendar_date();
                    Self::calendar_month(&ui, date.year, date.month);
                }
            }
        });
        ui.on_navigate_calendar({
            let weak = ui.as_weak();
            move |delta| {
                if let Some(ui) = weak.upgrade()
                    && let Some((year, month)) = crate::calendar::shift(
                        ui.get_calendar_year(),
                        ui.get_calendar_month(),
                        delta,
                    )
                {
                    Self::calendar_month(&ui, year, month);
                }
            }
        });
        ui.on_change_calendar_month({
            let weak = ui.as_weak();
            move |month| {
                if let Some(ui) = weak.upgrade()
                    && (1..=12).contains(&month)
                {
                    Self::calendar_month(&ui, ui.get_calendar_year(), month);
                }
            }
        });
        ui.on_change_calendar_year({
            let weak = ui.as_weak();
            move |year| {
                if let Some(ui) = weak.upgrade()
                    && (2000..=2099).contains(&year)
                {
                    Self::calendar_month(&ui, year, ui.get_calendar_month());
                }
            }
        });
        ui.on_pick_calendar_day({
            let weak = ui.as_weak();
            move |day| {
                if let Some(ui) = weak.upgrade() {
                    Self::choose_date(&ui, ui.get_calendar_year(), ui.get_calendar_month(), day);
                }
            }
        });
        ui.on_calendar_today({
            let weak = ui.as_weak();
            move || {
                if let Some(ui) = weak.upgrade() {
                    let mut today = unsafe { std::mem::zeroed() };
                    unsafe {
                        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut today);
                    }
                    Self::choose_date(
                        &ui,
                        today.wYear as i32,
                        today.wMonth as i32,
                        today.wDay as i32,
                    );
                }
            }
        });
        ui.on_choose_playback_date({
            let weak = ui.as_weak();
            move |year, month, day| {
                if let Some(ui) = weak.upgrade() {
                    Self::choose_date(&ui, year, month, day);
                }
            }
        });
        macro_rules! wire {($name:ident, |$this:ident,$window:ident $(,$arg:ident)*| $body:expr)=>{{let weak=ui.as_weak();let $this=self.clone();ui.$name(move |$($arg),*| {if let Some($window)=weak.upgrade(){$body}});}};}
        wire!(on_select_playback_device, |this, window, index| this
            .select(&window, index));
        wire!(
            on_toggle_playback_channel,
            |this, window, number, checked| this.toggle(&window, number, checked)
        );
        wire!(on_search_playback, |this, window| this.search(&window));
        wire!(on_seek_playback, |this, window, fraction| this
            .seek(&window, fraction));
        wire!(on_play_pause, |this, window| this.play_pause(&window));
        wire!(on_playback_speed_changed, |this, window, index| this
            .speed(&window, index));
        wire!(on_stop_playback, |this, window| this.stop(&window));
        wire!(on_prepare_export, |this, window| this
            .prepare_export(&window));
        wire!(on_export_playback, |this, window| this.export(&window));
        wire!(on_cancel_export, |this, window| this.cancel_export(&window));
        wire!(on_export_range_edited, |this, window| this
            .refresh_export_name(&window, false));
        ui.on_browse_playback_export({
            let weak = ui.as_weak();
            move || {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                let suggested = ui.get_export_path().to_string();
                let owner =
                    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }
                        as isize;
                let weak = weak.clone();
                std::thread::spawn(move || {
                    let path = crate::file_dialog::save_mp4(owner, &suggested);
                    let _ = weak.upgrade_in_event_loop(move |ui| match path {
                        Ok(Some(path)) => {
                            ui.set_export_path(path.to_string_lossy().into_owned().into())
                        }
                        Ok(None) => {}
                        Err(e) => ui.set_export_message(e.into()),
                    });
                });
            }
        });
    }
}
