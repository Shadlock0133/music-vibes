use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use eframe::{get_value, set_value, Storage};

use crate::util::SharedF32;

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct VibratorSettings {
    pub is_enabled: bool,
    pub multiplier: f32,
    pub min: f32,
    pub max: f32,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DeviceSettings {
    pub is_enabled: bool,
    pub multiplier: f32,
    pub min: f32,
    pub max: f32,
    pub vibrators: Vec<VibratorSettings>,
}

pub struct Settings {
    pub main_volume: f32,
    pub low_pass_freq: SharedF32,
    pub use_dark_mode: bool,
    pub start_scanning_on_startup: bool,
    pub polling_rate_ms: SharedF32,
    pub use_polling_rate: Arc<AtomicBool>,
    pub device_settings: HashMap<String, DeviceSettings>,
    pub save_device_settings: bool,
    pub enable_persistence: bool,
    pub hold_delay_ms: f32,
    pub decay_rate_per_sec: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            main_volume: defaults::MAIN_VOLUME,
            low_pass_freq: SharedF32::new(defaults::LOW_PASS_FREQ),
            use_dark_mode: defaults::DARK_MODE,
            start_scanning_on_startup: defaults::START_SCANNING_ON_STARTUP,
            polling_rate_ms: SharedF32::new(defaults::POLLING_RATE_MS),
            use_polling_rate: Arc::new(AtomicBool::new(
                defaults::USE_POLLING_RATE,
            )),
            device_settings: HashMap::new(),
            save_device_settings: false,
            enable_persistence: defaults::ENABLE_PERSISTENCE,
            hold_delay_ms: defaults::HOLD_DELAY_MS,
            decay_rate_per_sec: defaults::DECAY_RATE_PER_SEC,
        }
    }
}

mod names {
    pub const MAIN_VOLUME: &str = "main_volume";
    pub const LOW_PASS_FREQ: &str = "low_pass_freq";
    pub const DARK_MODE: &str = "dark_mode";
    pub const START_SCANNING_ON_STARTUP: &str = "start_scanning_on_startup";
    pub const POLLING_RATE_MS: &str = "polling_rate_ms";
    pub const USE_POLLING_RATE: &str = "use_polling_rate";
    pub const DEVICE_SETTINGS: &str = "device_settings";
    pub const SAVE_DEVICE_SETTINGS: &str = "save_device_settings";
    pub const ENABLE_PERSISTENCE: &str = "enable_persistence";
    pub const HOLD_DELAY_MS: &str = "hold_delay_ms";
    pub const DECAY_RATE_PER_SEC: &str = "decay_rate_per_sec";
}

pub mod defaults {
    pub const MAIN_VOLUME: f32 = 1.0;
    pub const LOW_PASS_FREQ: f32 = 20_000.0;
    pub const DARK_MODE: bool = true;
    pub const START_SCANNING_ON_STARTUP: bool = false;
    pub const POLLING_RATE_MS: f32 = 20.0;
    pub const USE_POLLING_RATE: bool = false;
    pub const ENABLE_PERSISTENCE: bool = false;
    pub const HOLD_DELAY_MS: f32 = 0.0;
    pub const DECAY_RATE_PER_SEC: f32 = 2.0;
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
        get_value(storage, names::START_SCANNING_ON_STARTUP)
            .unwrap_or(defaults::START_SCANNING_ON_STARTUP);
        let polling_rate_ms = get_value(storage, names::POLLING_RATE_MS)
            .unwrap_or(defaults::POLLING_RATE_MS);
        let use_polling_rate = get_value(storage, names::USE_POLLING_RATE)
            .unwrap_or(defaults::USE_POLLING_RATE);
        let device_settings: HashMap<String, DeviceSettings> =
            get_value(storage, names::DEVICE_SETTINGS).unwrap_or_default();
        let save_device_settings =
            get_value(storage, names::SAVE_DEVICE_SETTINGS).unwrap_or(false);
        let enable_persistence = get_value(storage, names::ENABLE_PERSISTENCE)
            .unwrap_or(defaults::ENABLE_PERSISTENCE);
        let hold_delay_ms = get_value(storage, names::HOLD_DELAY_MS)
            .unwrap_or(defaults::HOLD_DELAY_MS);
        let decay_rate_per_sec = get_value(storage, names::DECAY_RATE_PER_SEC)
            .unwrap_or(defaults::DECAY_RATE_PER_SEC);
        Self {
            main_volume,
            low_pass_freq: SharedF32::new(low_pass_freq),
            use_dark_mode,
            start_scanning_on_startup,
            polling_rate_ms: SharedF32::new(polling_rate_ms),
            use_polling_rate: Arc::new(AtomicBool::new(use_polling_rate)),
            device_settings,
            save_device_settings,
            enable_persistence,
            hold_delay_ms,
            decay_rate_per_sec,
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
        set_value(
            storage,
            names::POLLING_RATE_MS,
            &self.polling_rate_ms.load(),
        );
        set_value(
            storage,
            names::USE_POLLING_RATE,
            &self.use_polling_rate.load(Ordering::Relaxed),
        );
        set_value(
            storage,
            names::SAVE_DEVICE_SETTINGS,
            &self.save_device_settings,
        );
        set_value(storage, names::ENABLE_PERSISTENCE, &self.enable_persistence);
        set_value(storage, names::HOLD_DELAY_MS, &self.hold_delay_ms);
        set_value(storage, names::DECAY_RATE_PER_SEC, &self.decay_rate_per_sec);
        if self.save_device_settings {
            set_value(storage, names::DEVICE_SETTINGS, &self.device_settings);
        }
    }
}
