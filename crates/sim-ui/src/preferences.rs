use serde::{Deserialize, Serialize};
use sim_core::Device;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub restore: bool,
    pub startup: bool,
    pub gpu: bool,
    pub view: View,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            restore: true,
            startup: false,
            gpu: true,
            view: View::default(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    pub quality: Option<i32>,
    pub layout: i32,
    pub layout_preset: Option<usize>,
    pub selected: i32,
    pub expanded: bool,
    pub slots: Vec<Option<SavedSlot>>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            quality: None,
            layout: 4,
            layout_preset: None,
            selected: 0,
            expanded: false,
            slots: Vec::new(),
        }
    }
}
impl View {
    pub fn quality(&self) -> crate::mosaic::QualityMode {
        crate::mosaic::QualityMode::from_index(
            self.quality
                .or_else(|| self.slots.iter().flatten().next().map(|s| s.quality))
                .unwrap_or(1),
        )
    }
    pub fn preset(&self) -> usize {
        crate::layouts::index(self.layout_preset, self.layout)
    }
    pub fn layout(&self) -> i32 {
        crate::layouts::COUNTS[self.preset()]
    }
    pub fn selected(&self) -> i32 {
        self.selected.clamp(0, self.layout() - 1)
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SavedSlot {
    pub device: i64,
    pub address: String,
    pub channel: u16,
    pub quality: i32,
}
impl SavedSlot {
    pub fn resolve<'a>(&self, devices: &'a [Device]) -> Option<&'a Device> {
        devices.iter().find(|d| {
            d.id == self.device
                && d.config.address == self.address
                && d.config.channels.iter().any(|c| c.number == self.channel)
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_hidden_positions_and_distinguishes_presets_with_the_same_count() {
        for preset in 0..crate::layouts::COUNTS.len() {
            let view = View {
                layout: crate::layouts::COUNTS[preset],
                layout_preset: Some(preset),
                ..Default::default()
            };
            let loaded: View =
                serde_json::from_str(&serde_json::to_string(&view).unwrap()).unwrap();
            assert_eq!(loaded.preset(), preset);
        }
        let view = View {
            layout: 1,
            slots: (0..4)
                .map(|channel| {
                    Some(SavedSlot {
                        device: 1,
                        address: "fixture".into(),
                        channel,
                        quality: 1,
                    })
                })
                .collect(),
            ..Default::default()
        };
        let loaded: View = serde_json::from_str(&serde_json::to_string(&view).unwrap()).unwrap();
        assert_eq!(loaded.layout(), 1);
        assert_eq!(loaded.slots.iter().flatten().count(), 4);
    }
    #[test]
    fn defaults_and_invalid_layout_are_bounded() {
        let p: Preferences = serde_json::from_str("{}").unwrap();
        assert!(p.gpu && p.restore);
        assert!(!p.startup);
        let view: View = serde_json::from_str(r#"{"layout":99,"selected":100}"#).unwrap();
        assert_eq!(view.layout(), 4);
        assert_eq!(view.selected(), 3);
    }
    #[test]
    fn restores_one_global_quality_and_maps_legacy_auto_to_sd() {
        use crate::mosaic::QualityMode;
        let mut view = View {
            slots: vec![
                None,
                Some(SavedSlot {
                    device: 1,
                    address: "fixture".into(),
                    channel: 0,
                    quality: 2,
                }),
            ],
            ..Default::default()
        };
        assert_eq!(view.quality(), QualityMode::Hd);
        view.quality = Some(1);
        assert_eq!(view.quality(), QualityMode::Sd);
        view.quality = Some(2);
        assert_eq!(view.quality(), QualityMode::Hd);
        view.quality = None;
        view.slots[1].as_mut().unwrap().quality = 0;
        assert_eq!(view.quality(), QualityMode::Sd);
    }
    #[test]
    fn serializes_empty_positions_and_quality_without_credentials() {
        let view = View {
            slots: vec![
                None,
                Some(SavedSlot {
                    device: 7,
                    address: "TESTSERIAL".into(),
                    channel: 2,
                    quality: 2,
                }),
            ],
            ..Default::default()
        };
        let json = serde_json::to_string(&view).unwrap();
        let loaded: View = serde_json::from_str(&json).unwrap();
        assert!(loaded.slots[0].is_none());
        assert_eq!(loaded.slots[1].as_ref().unwrap().quality, 2);
        assert!(loaded.slots[1].as_ref().unwrap().resolve(&[]).is_none());
        assert!(!json.contains("password"));
    }
    #[test]
    fn ignores_removed_channel_or_reused_device_id() {
        let mut d = Device {
            id: 7,
            config: sim_core::DeviceDraft {
                serial: None,
                connection: sim_core::ConnectionMode::P2p,
                name: "Fixture".into(),
                address: "TESTSERIAL".into(),
                port: 37777,
                username: "fixture".into(),
                manufacturer: sim_core::Manufacturer::Dahua,
                channels: vec![sim_core::Channel {
                    number: 2,
                    name: "Entrada".into(),
                }],
            },
        };
        let saved = SavedSlot {
            device: 7,
            address: "TESTSERIAL".into(),
            channel: 2,
            quality: 0,
        };
        assert!(saved.resolve(&[d.clone()]).is_some());
        d.config.address = "OTHER".into();
        assert!(saved.resolve(&[d.clone()]).is_none());
        d.config.address = "TESTSERIAL".into();
        d.config.channels.clear();
        assert!(saved.resolve(&[d]).is_none());
    }
}
