use eframe::{get_value, set_value, Storage};

use crate::util::SharedF32;

pub struct Settings {
    pub low_pass_freq: SharedF32,
    pub use_dark_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            low_pass_freq: SharedF32::new(defaults::LOW_PASS_FREQ),
            use_dark_mode: defaults::DARK_MODE,
        }
    }
}

mod names {
    pub const LOW_PASS_FREQ: &str = "low_pass_freq";
    pub const DARK_MODE: &str = "dark_mode";
}
mod defaults {
    pub const LOW_PASS_FREQ: f32 = 20_000.0;
    pub const DARK_MODE: bool = true;
}

impl Settings {
    pub fn load(storage: &dyn Storage) -> Self {
        let low_pass_freq = get_value(storage, names::LOW_PASS_FREQ)
            .unwrap_or(defaults::LOW_PASS_FREQ);
        let use_dark_mode =
            get_value(storage, names::DARK_MODE).unwrap_or(defaults::DARK_MODE);
        Self {
            low_pass_freq: SharedF32::new(low_pass_freq),
            use_dark_mode,
        }
    }

    pub fn save(&self, storage: &mut dyn Storage) {
        set_value(storage, names::LOW_PASS_FREQ, &self.low_pass_freq.load());
        set_value(storage, names::DARK_MODE, &self.use_dark_mode);
    }
}
