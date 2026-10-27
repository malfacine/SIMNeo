use crate::{AppWindow, video::Controller};
use sim_core::{Channel, ConnectionMode, DeviceDraft, Manufacturer};
use sim_storage_import::NewDevice;
use std::cell::{Cell, RefCell};

struct Input {
    device: NewDevice,
    existing: Option<i64>,
}
impl Input {
    fn read(ui: &AppWindow) -> Result<Self, String> {
        let ip = ui.get_form_use_ip();
        let serial = ui.get_form_use_serial();
        if !ip && !serial {
            return Err("Marque IP, serial ou ambos para testar.".into());
        }
        let port = ui
            .get_form_port()
            .trim()
            .parse::<u16>()
            .map_err(|_| "Confira a porta de serviço (1–65535).")?;
        let config = DeviceDraft {
            name: "Teste de conexão".into(),
            connection: if ip {
                ConnectionMode::Address
            } else {
                ConnectionMode::P2p
            },
            address: if ip {
                ui.get_form_address()
            } else {
                ui.get_form_serial()
            }
            .trim()
            .into(),
            serial: if ip && serial {
                Some(ui.get_form_serial().trim().into())
            } else {
                None
            },
            port,
            username: ui.get_form_user().trim().into(),
            manufacturer: if ui.get_form_maker() == 0 {
                Manufacturer::Dahua
            } else {
                Manufacturer::Jufeng
            },
            channels: vec![],
        };
        config.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            device: NewDevice::new(config, ui.get_form_password().to_string()),
            existing: ui.get_edit_id().parse().ok(),
        })
    }
    fn same(&self, other: &Self) -> bool {
        self.existing == other.existing
            && self.device.config == other.device.config
            && self.device.password.as_str() == other.device.password.as_str()
    }
}
#[derive(Default)]
pub struct Tester {
    next: Cell<u64>,
    active: RefCell<Option<(u64, Input)>>,
    tested: RefCell<Option<(Input, Vec<Channel>)>>,
}
impl Tester {
    pub fn cancel(&self, ui: &AppWindow, video: &Controller) {
        if let Some((token, _)) = self.active.borrow_mut().take() {
            video.cancel_probe(token);
        }
        self.tested.borrow_mut().take();
        ui.set_testing_device(false);
        ui.set_probe_message("".into());
    }
    pub fn start(&self, ui: &AppWindow, video: &Controller) {
        self.cancel(ui, video);
        let input = match Input::read(ui) {
            Ok(input) => input,
            Err(message) => {
                ui.set_probe_message(message.into());
                return;
            }
        };
        let token = self.next.get().wrapping_add(1);
        self.next.set(token);
        let request = NewDevice::new(
            input.device.config.clone(),
            input.device.password.to_string(),
        );
        if !video.probe(token, request, input.existing) {
            ui.set_probe_message("Não foi possível iniciar o teste de conexão.".into());
            return;
        }
        *self.active.borrow_mut() = Some((token, input));
        ui.set_testing_device(true);
        ui.set_probe_message("Testando conexão e consultando canais…".into());
    }
    pub fn result(
        &self,
        ui: &AppWindow,
        token: u64,
        result: Result<(Vec<Channel>, String), String>,
    ) {
        if !self
            .active
            .borrow()
            .as_ref()
            .is_some_and(|(current, _)| *current == token)
        {
            return;
        }
        let (_, input) = self.active.borrow_mut().take().unwrap();
        ui.set_testing_device(false);
        if ui.get_dialog() != "device" {
            return;
        }
        if !Input::read(ui).is_ok_and(|current| input.same(&current)) {
            ui.set_probe_message("Os dados foram alterados. Teste a conexão novamente.".into());
            return;
        }
        match result {
            Ok((channels, message)) => {
                ui.set_form_count(channels.len().to_string().into());
                *self.tested.borrow_mut() = Some((input, channels));
                ui.set_probe_message(message.into());
            }
            Err(message) => ui.set_probe_message(format!("Falha no teste: {message}").into()),
        }
    }
    pub fn channels(&self, ui: &AppWindow, count: u16) -> Option<Vec<Channel>> {
        let current = Input::read(ui).ok()?;
        let tested = self.tested.borrow();
        let (input, channels) = tested.as_ref()?;
        (input.same(&current) && channels.len() == usize::from(count)).then(|| channels.clone())
    }
}
