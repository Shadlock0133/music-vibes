use eframe::{get_value, set_value, Storage};

use crate::util::SharedF32;

// TODO: Add derive macro
pub struct Settings {
    pub main_volume: f32,
    pub low_pass_freq: SharedF32,
    pub use_dark_mode: bool,
    pub start_scanning_on_startup: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            main_volume: defaults::MAIN_VOLUME,
            low_pass_freq: SharedF32::new(defaults::LOW_PASS_FREQ),
            use_dark_mode: defaults::DARK_MODE,
            start_scanning_on_startup: defaults::START_SCANNING_ON_STARTUP,
        }
    }
}

mod names {
    pub const MAIN_VOLUME: &str = "main_volume";
    pub const LOW_PASS_FREQ: &str = "low_pass_freq";
    pub const DARK_MODE: &str = "dark_mode";
    pub const START_SCANNING_ON_STARTUP: &str = "start_scanning_on_startup";
}
mod defaults {
    pub const MAIN_VOLUME: f32 = 1.0;
    pub const LOW_PASS_FREQ: f32 = 20_000.0;
    pub const DARK_MODE: bool = true;
    pub const START_SCANNING_ON_STARTUP: bool = false;
}

impl Settings {
    pub fn load(storage: &dyn Storage) -> Self {
        let main_volume = get_value(storage, names::MAIN_VOLUME)
            .unwrap_or(defaults::MAIN_VOLUME);
        let low_pass_freq = get_value(storage, names::LOW_PASS_FREQ)
            .unwrap_or(defaults::LOW_PASS_FREQ);
        let use_dark_mode =
            get_value(storage, names::DARK_MODE).unwrap_or(defaults::DARK_MODE);
        let start_scanning_on_startup =
            get_value(storage, names::START_SCANNING_ON_STARTUP)
                .unwrap_or(defaults::START_SCANNING_ON_STARTUP);
        Self {
            main_volume,
            low_pass_freq: SharedF32::new(low_pass_freq),
            use_dark_mode,
            start_scanning_on_startup,
        }
    }

    pub fn save(&self, storage: &mut dyn Storage) {
        set_value(storage, names::MAIN_VOLUME, &self.main_volume);
        set_value(storage, names::LOW_PASS_FREQ, &self.low_pass_freq.load());
        set_value(storage, names::DARK_MODE, &self.use_dark_mode);
        set_value(
            storage,
            names::START_SCANNING_ON_STARTUP,
            &self.start_scanning_on_startup,
        );
    }
}
