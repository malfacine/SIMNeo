use sim_media::StreamQuality;

pub const CAPACITY: usize = 64;

pub fn first_channels(channels: &[sim_core::Channel], layout: i32) -> Vec<sim_core::Channel> {
    let mut channels = channels.to_vec();
    channels.sort_by_key(|channel| channel.number);
    channels.truncate(layout.clamp(1, CAPACITY as i32) as usize);
    channels
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QualityMode {
    #[default]
    Sd,
    Hd,
}
impl QualityMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            2 => Self::Hd,
            _ => Self::Sd,
        }
    }
    pub fn index(self) -> i32 {
        match self {
            Self::Sd => 1,
            Self::Hd => 2,
        }
    }
    pub fn resolve(self) -> StreamQuality {
        match self {
            Self::Hd => StreamQuality::Hd,
            Self::Sd => StreamQuality::Sd,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Binding {
    pub device: i64,
    pub channel: u16,
    pub title: String,
}
#[derive(Default)]
pub struct Slot {
    pub binding: Option<Binding>,
    pub generation: u64,
    pub mode: QualityMode,
    pub quality: StreamQuality,
}
impl Slot {
    pub fn assign(&mut self, binding: Binding, mode: QualityMode) {
        self.binding = Some(binding);
        self.mode = mode;
        self.quality = self.mode.resolve();
        self.invalidate();
    }
    pub fn clear(&mut self) {
        self.binding = None;
        self.invalidate();
    }
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn set_quality(&mut self, mode: QualityMode) -> bool {
        let quality = mode.resolve();
        self.mode = mode;
        if self.binding.is_some() && self.quality != quality {
            self.quality = quality;
            self.invalidate();
            true
        } else {
            false
        }
    }
    pub fn accepts(&self, generation: u64) -> bool {
        self.binding.is_some() && self.generation == generation
    }
}
pub fn retry_delay(failures: u32) -> std::time::Duration {
    std::time::Duration::from_secs((2u64.saturating_pow(failures.min(4))).clamp(2, 20))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dvr_click_opens_first_channels_in_numeric_order_up_to_layout() {
        let channels: Vec<_> = (0..18)
            .rev()
            .map(|number| sim_core::Channel {
                number,
                name: format!("CAM {}", number + 1),
            })
            .collect();
        assert_eq!(
            first_channels(&channels, 4)
                .iter()
                .map(|c| c.number)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(first_channels(&channels, 16).len(), 16);
        assert_eq!(first_channels(&channels[..2], 4).len(), 2);
    }
    fn binding(device: i64, channel: u16) -> Binding {
        Binding {
            device,
            channel,
            title: "Fixture".into(),
        }
    }
    #[test]
    fn replacement_and_close_reject_stale_frames_without_affecting_neighbors() {
        let mut slots: [Slot; CAPACITY] = std::array::from_fn(|_| Slot::default());
        slots[0].assign(binding(1, 0), QualityMode::Sd);
        slots[1].assign(binding(2, 3), QualityMode::Sd);
        let old = slots[0].generation;
        let other = slots[1].generation;
        slots[0].assign(binding(2, 5), QualityMode::Sd);
        assert!(!slots[0].accepts(old));
        slots[0].clear();
        assert!(!slots[0].accepts(slots[0].generation));
        assert!(slots[1].accepts(other));
        assert_eq!(slots[1].binding.as_ref().unwrap().channel, 3);
    }
    #[test]
    fn quality_is_explicit_and_applies_to_each_bound_slot() {
        let mut slots: [Slot; CAPACITY] = std::array::from_fn(|_| Slot::default());
        slots[0].assign(binding(1, 0), QualityMode::Sd);
        slots[1].assign(binding(2, 1), QualityMode::Sd);
        for s in slots.iter_mut().take(2) {
            assert!(s.set_quality(QualityMode::Hd));
            assert_eq!(s.quality, StreamQuality::Hd);
            assert!(!s.set_quality(QualityMode::Hd));
        }
        slots[2].assign(binding(1, 2), QualityMode::Hd);
        assert_eq!(slots[2].quality, StreamQuality::Hd);
        for s in slots.iter_mut().take(3) {
            assert!(s.set_quality(QualityMode::Sd));
        }
        assert_eq!(QualityMode::from_index(0), QualityMode::Sd);
    }
    #[test]
    fn retry_backoff_is_bounded() {
        assert_eq!(retry_delay(0).as_secs(), 2);
        assert_eq!(retry_delay(100).as_secs(), 16);
    }
}
