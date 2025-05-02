use std::{
    collections::{HashMap, VecDeque},
    iter::from_fn,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle, time::Instant,
    time::Duration,
};

use audio_capture::win::capture::AudioCapture;
use buttplug::{
    client::{ButtplugClient, ButtplugClientDevice, VibrateCommand, ButtplugClientError},
    core::message::ActuatorType,
};
use clap::Parser;
use eframe::{
    egui::{
        self, Button, Color32, ProgressBar, RichText, SelectableLabel, Slider,
        TextFormat, Ui, Visuals, Window,
    },
    epaint::text::LayoutJob,
    CreationContext, Storage,
};
use tokio::runtime::Runtime;

use crate::{
    settings::{defaults, Settings, DeviceSettings, VibratorSettings},
    util::{self, MinCutoff, SharedF32},
};

#[derive(Parser, Default)]
pub struct Gui {
    #[clap(short, long)]
    server_addr: Option<String>,
}

pub fn gui(args: Gui) {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Music Vibes",
        native_options,
        Box::new(|ctx| Box::new(GuiApp::new(args.server_addr, ctx))),
    );
}

#[allow(dead_code)]
enum ConnectionState {
    Disconnected, // Necessary in case of server disconnect
    Connecting,
    Connected,
    Error(String),
}

// Stop devices on program shutdown
impl Drop for GuiApp {
    fn drop(&mut self) {
        if let Some(client) = &self.client {
            if let Err(e) = self.runtime.block_on(client.stop_all_devices()) {
                eprintln!("Error stopping devices: {:?}", e);
            }
        }
    }
}

struct GuiApp {
    runtime: tokio::runtime::Runtime,
    client: Option<ButtplugClient>,
    connection_state: ConnectionState,
    connection_task: Option<tokio::task::JoinHandle<Result<ButtplugClient, ButtplugClientError>>>,
    server_addr: Option<String>,
    devices: HashMap<String, DeviceProps>,
    sound_power: SharedF32,
    _capture_thread: JoinHandle<()>,
    is_scanning: bool,
    show_settings: bool,
    vibration_level: f32,
    hold_start_time: Option<Instant>,
    settings: Settings, // Persistent settings
}

struct DeviceProps {
    is_enabled: bool,
    battery_state: BatteryState,
    multiplier: f32,
    min: f32,
    max: f32,
    vibrators: Vec<VibratorProps>,
}

// TEMP: if readout returned an error, SharedF32 will be set to NaN
#[allow(dead_code)]
struct BatteryState(SharedF32, tokio::task::JoinHandle<()>);

impl BatteryState {
    pub fn new(runtime: &Runtime, device: Arc<ButtplugClientDevice>) -> Self {
        let shared_level = SharedF32::new(0.0);
        let task = {
            let shared_level = shared_level.clone();
            runtime.spawn(battery_check_bg_task(device, shared_level))
        };
        Self(shared_level, task)
    }

    pub fn get_level(&self) -> Option<f32> {
        let value = self.0.load();
        if value.is_nan() {
            None
        } else {
            Some(value)
        }
    }
}

async fn battery_check_bg_task(
    device: Arc<ButtplugClientDevice>,
    shared_level: SharedF32,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        match device.battery_level().await {
            Ok(level) => shared_level.store(level as f32),
            Err(_) => {
                shared_level.store(f32::NAN);
                break;
            }
        }
    }
}

impl DeviceProps {
    fn new(runtime: &Runtime, device: Arc<ButtplugClientDevice>, settings: &Settings) -> Self {
        let vibe_count = device
            .message_attributes()
            .scalar_cmd()
            .as_ref()
            .map(|x| {
                x.iter()
                    .filter(|x| x.actuator_type() == &ActuatorType::Vibrate)
                    .count()
            })
            .unwrap_or_default();

        let device_settings = if settings.save_device_settings {
            settings.device_settings.get(device.name().as_str())
        } else {
            None
        };

        let (_is_enabled, multiplier, min, max, vibrators) = if let Some(ds) = device_settings {
            let mut vib_props = Vec::new();
            for vs in &ds.vibrators {
                vib_props.push(VibratorProps {
                    is_enabled: vs.is_enabled,
                    multiplier: vs.multiplier,
                    min: vs.min,
                    max: vs.max,
                });
            }
            while vib_props.len() < vibe_count {
                vib_props.push(VibratorProps::default());
            }
            (
                ds.is_enabled,
                ds.multiplier,
                ds.min,
                ds.max,
                vib_props,
            )
        } else {
            let vibrators = from_fn(|| Some(VibratorProps::default()))
                .take(vibe_count)
                .collect();
            (false, 1.0, 0.0, 1.0, vibrators)
        };
        Self {
            is_enabled: false, // Start device disabled
            battery_state: BatteryState::new(runtime, device),
            multiplier,
            min,
            max,
            vibrators,
        }
    }
}

impl DeviceProps {
    fn calculate_visual_output(&self, input: f32) -> (f32, bool) {
        let power = (input * self.multiplier).clamp(0.0, self.max);
        (power, power < self.min)
    }

    fn calculate_output(&self, input: f32) -> f32 {
        (input * self.multiplier)
            .clamp(0.0, self.max)
            .min_cutoff(self.min)
    }
}

fn capture_thread(
    sound_power: SharedF32,
    low_pass_freq: SharedF32,
    polling_rate_ms: SharedF32,
    use_polling_rate: Arc<AtomicBool>,
) -> ! {
    let dur = Duration::from_millis(1);
    let mut capture = AudioCapture::init(dur).unwrap();

    let format = capture.format().unwrap();
    // time to fill about half of AudioCapture's buffer
    let actual_duration = Duration::from_secs_f32(
        dur.as_secs_f32() * capture.buffer_frame_size as f32
            / format.sample_rate as f32
            / 1000.,
    ) / 2;

    let buffer_duration = Duration::from_millis(20);
    let buffer_size = (format.sample_rate as f32
        * buffer_duration.as_secs_f32()) as usize
        * format.channels as usize;
    let mut buf = VecDeque::new();
    buf.resize(buffer_size, 0.0);

    capture.start().unwrap();
    loop {
        let use_custom = use_polling_rate.load(Ordering::Relaxed);
        let sleep_duration = if use_custom {
            Duration::from_millis(polling_rate_ms.load().max(1.0) as u64)
        } else {
            actual_duration
        };
        std::thread::sleep(sleep_duration);

        capture
            .read_samples::<(), _>(|samples, _| {
                for value in samples {
                    buf.push_front(*value);
                }
                buf.truncate(buffer_size);
                Ok(())
            })
            .unwrap();

        let buf = buf.make_contiguous();
        let rc = 1.0 / low_pass_freq.load();
        let filtered = util::low_pass(buf, dur, rc, format.channels as _);
        let speeds = util::calculate_power(&filtered, format.channels as _);
        let avg = util::avg(&speeds).clamp(0.0, 1.0);
        sound_power.store(avg);
    }
}

impl GuiApp {
    fn new(server_addr: Option<String>, ctx: &CreationContext) -> Self {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = None;
        let devices = Default::default();

        let connection_state = ConnectionState::Connecting;
        let server_addr_clone = server_addr.clone();
        let connection_task = Some(runtime.spawn(async move {
            util::start_bp_server(server_addr_clone).await
        }));

        let sound_power = SharedF32::new(0.0);
        let sound_power2 = sound_power.clone();

        let settings = ctx.storage.map(Settings::load).unwrap_or_default();
        let low_pass_freq = settings.low_pass_freq.clone();
        let polling_rate_ms = settings.polling_rate_ms.clone();
        let use_polling_rate = settings.use_polling_rate.clone();

        let _capture_thread = std::thread::spawn(move || {
            capture_thread(
                sound_power2,
                low_pass_freq,
                polling_rate_ms,
                use_polling_rate,
            )
        });

        let is_scanning = false;

        GuiApp {
            runtime,
            client,
            connection_state,
            connection_task,
            server_addr,
            devices,
            sound_power,
            _capture_thread,
            is_scanning,
            show_settings: false,
            settings,
            vibration_level: 0.0,
            hold_start_time: None,
        }
    }
}

impl eframe::App for GuiApp {
    fn save(&mut self, storage: &mut dyn Storage) {
        // Update device settings before saving if the toggle is enabled
        if self.settings.save_device_settings {
            for (device_name, props) in &self.devices {
                let mut vibrators = Vec::new();
                for vibe in &props.vibrators {
                    vibrators.push(VibratorSettings {
                        is_enabled: vibe.is_enabled,
                        multiplier: vibe.multiplier,
                        min: vibe.min,
                        max: vibe.max,
                    });
                }
                let device_settings = DeviceSettings {
                    is_enabled: false,  // Start device disabled
                    multiplier: props.multiplier,
                    min: props.min,
                    max: props.max,
                    vibrators,
                };
                self.settings.device_settings.insert(device_name.clone(), device_settings);
            }
        }
        self.settings.save(storage);
        storage.flush();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let visuals = match self.settings.use_dark_mode {
            true => Visuals::dark(),
            false => Visuals::light(),
        };
        ctx.set_visuals(visuals);

        // --- Connection Handling ---
        if let Some(task) = self.connection_task.take() {
            if task.is_finished() {
                match self.runtime.block_on(task) {
                    Ok(Ok(client)) => {
                        self.client = Some(client);
                        self.connection_state = ConnectionState::Connected;
                        if self.settings.start_scanning_on_startup {
                            if let Some(client) = &self.client {
                                self.runtime.spawn(client.start_scanning());
                                self.is_scanning = true;
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        self.connection_state = ConnectionState::Error(format!("Connection failed: {}", e));
                    }
                    Err(e) => {
                        self.connection_state = ConnectionState::Error(format!("Connection task panicked unexpectedly: {:?}", e));
                    }
                }
            } else {
                self.connection_task = Some(task);
            }
        }
        // --- End Connection Handling ---

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                if !matches!(self.connection_state, ConnectionState::Connecting) {
                    match self.connection_state {
                        ConnectionState::Disconnected | ConnectionState::Error(_) => {
                            if ui.button("Connect to Server").clicked() {
                                self.connection_state = ConnectionState::Connecting;
                                let server_addr_clone = self.server_addr.clone();
                                self.connection_task = Some(self.runtime.spawn(async move {
                                    util::start_bp_server(server_addr_clone).await
                                }));
                            }
                        }
                        ConnectionState::Connected => {
                            if let Some(client) = &self.client {
                                let scan_label = if self.is_scanning {
                                    "Stop scanning"
                                } else {
                                    "Start scanning"
                                };
                                if ui.selectable_label(self.is_scanning, scan_label).clicked() {
                                    if self.is_scanning {
                                        self.runtime.spawn(client.stop_scanning());
                                        self.is_scanning = false;
                                    } else {
                                        self.runtime.spawn(client.start_scanning());
                                        self.is_scanning = true;
                                    }
                                }
                            }
                        }
                        ConnectionState::Connecting => {}
                    }
                }
                
                match &self.connection_state {
                    ConnectionState::Disconnected => {
                        ui.label("Disconnected");
                    }
                    ConnectionState::Connecting => {
                        ui.label("Connecting...");
                    }
                    ConnectionState::Error(_msg) => {
                        ui.colored_label(Color32::RED, "Error");
                    }
                    ConnectionState::Connected => { /* No status needed when connected */ }
                }

                if ui.button("Settings").clicked() {
                    self.show_settings = true;
                }

                let stop_button_width = 120.0;
                ui.add_space(ui.available_width() - stop_button_width);

                let stop_button = Button::new(
                    RichText::new("Stop all devices").color(Color32::BLACK),
                )
                .fill(Color32::from_rgb(240, 0, 0));
                if ui
                    .add_sized([stop_button_width, 30.0], stop_button)
                    .clicked()
                {
                    if let Some(client) = &self.client {
                        self.runtime.spawn(client.stop_all_devices());
                        for device in self.devices.values_mut() {
                            device.is_enabled = false;
                        }
                    }
                }
            });

            ui.separator();
            
            let delta_time = ctx.input().stable_dt;
            let main_mul = self.settings.main_volume.powi(2);
            let sound_power =
                (self.sound_power.load() * main_mul).clamp(0.0, 1.0);

            // --- Calculate Persistent Vibration Level ---
            let mut persistent_level = self.vibration_level;

            if !self.settings.enable_persistence {
                persistent_level = sound_power;
                self.hold_start_time = None;
            } else {
                if sound_power >= persistent_level {
                    persistent_level = sound_power;
                    self.hold_start_time = None;
                } else {
                    match self.hold_start_time {
                        None => {
                            if self.settings.hold_delay_ms >= 1.0 {
                                self.hold_start_time = Some(Instant::now());
                            } else {
                                let decay_rate_per_sec = self.settings.decay_rate_per_sec;
                                if decay_rate_per_sec <= 0.0 {
                                    persistent_level = sound_power;
                                } else {
                                    let decay_amount = decay_rate_per_sec * delta_time;
                                    persistent_level -= decay_amount;
                                    persistent_level = persistent_level.max(sound_power);
                                }
                            }
                        }
                        Some(start_time) => {
                            let hold_duration = Duration::from_millis(self.settings.hold_delay_ms as u64);
                            if start_time.elapsed() >= hold_duration {
                                let decay_rate_per_sec = self.settings.decay_rate_per_sec;
                                if decay_rate_per_sec <= 0.0 {
                                    persistent_level = sound_power;
                                    self.hold_start_time = None;
                                } else {
                                    let decay_amount = decay_rate_per_sec * delta_time;
                                    persistent_level -= decay_amount;
                                    persistent_level = persistent_level.max(sound_power);
                                }
                            }
                        }
                    }
                }
                persistent_level = persistent_level.max(0.0);
            }
            self.vibration_level = persistent_level.clamp(0.0, 1.0);
            // --- End Persistent Vibration Level Calculation ---

            ui.horizontal(|ui| {
                ui.label(format!(
                    "Current Output: {:.2}%",
                    self.vibration_level * 100.0
                ));
                ui.add(ProgressBar::new(self.vibration_level));
            });

            ui.horizontal(|ui| {
                // Main Volume Slider
                let r1 = ui.label("Main volume: ");
                let mut volume_as_percent = self.settings.main_volume * 100.0;
                let slider_response = ui.add(
                    Slider::new(&mut volume_as_percent, 0.0..=500.0)
                        .suffix("%"),
                );
                if slider_response.changed() {
                    self.settings.main_volume = volume_as_percent / 100.0;
                }
                if slider_response.double_clicked() {
                    self.settings.main_volume = defaults::MAIN_VOLUME;
                }
                let mut text = LayoutJob::default();
                text.append(
                    "Controls global volume level\n",
                    0.0,
                    TextFormat::default(),
                );
                text.append(
                    "Warning!!!",
                    0.0,
                    TextFormat {
                        color: Color32::RED,
                        ..Default::default()
                    },
                );
                text.append(
                    " Be careful, it's exponential so 200% is 4 times stronger!",
                    0.0,
                    TextFormat::default(),
                );
                r1.union(slider_response).on_hover_text_at_pointer(text);

                // Low Pass Freq Slider
                let mut low_pass_freq = self.settings.low_pass_freq.load();
                let r1 = ui.label("Low pass freq.: ");
                let slider_response = ui.add(
                    Slider::new(&mut low_pass_freq, 0.0..=20_000.0)
                        .logarithmic(true)
                        .integer(),
                );
                if slider_response.changed() {
                    self.settings.low_pass_freq.store(low_pass_freq);
                }
                if slider_response.double_clicked() {
                    self.settings
                        .low_pass_freq
                        .store(defaults::LOW_PASS_FREQ);
                }
                r1.union(slider_response).on_hover_text_at_pointer(
                    "Filters out frequencies above this one,\n\
                    leaving only lower frequencies.\n\
                    Defaults to max (20_000 Hz)",
                );
                // Polling Rate Slider
                let is_custom_polling_enabled = self.settings.use_polling_rate.load(Ordering::Relaxed);
                if is_custom_polling_enabled {
                    let r1 = ui.label("Polling Rate: ");
                    let mut polling_rate = self.settings.polling_rate_ms.load();
                    let slider_response = ui.add(
                        Slider::new(&mut polling_rate, 1.0..=500.0)
                            .integer()
                            .logarithmic(true),
                    );
                    if slider_response.changed() {
                        self.settings.polling_rate_ms.store(polling_rate);
                    }
                    if slider_response.double_clicked() {
                        self.settings
                            .polling_rate_ms
                            .store(defaults::POLLING_RATE_MS);
                    }
                    r1.union(slider_response).on_hover_text_at_pointer(
                        "Controls how often audio is analyzed (in milliseconds).\n\
                        Useful if your device is lagging.",
                    );
                }
            });
            // --- Persistence Controls ---
            ui.horizontal(|ui| {
                if self.settings.enable_persistence {
                    ui.horizontal(|ui| {
                        // Hold Delay Slider
                        let r1 = ui.label("Hold Delay: ");
                        let slider_response = ui.add(
                            Slider::new(&mut self.settings.hold_delay_ms, 0.0..=500.0)
                                .integer()
                                .logarithmic(true),
                        );
                        if slider_response.double_clicked() {
                            self.settings.hold_delay_ms = defaults::HOLD_DELAY_MS;
                        }
                        r1.union(slider_response).on_hover_text_at_pointer("Time to hold peak vibration after sound drops (in milliseconds).");

                        // Decay Rate Slider                       
                        let r1 = ui.label("Decay Rate: ");
                        let slider_response = ui.add(
                            Slider::new(&mut self.settings.decay_rate_per_sec, 0.01..=4.0)
                            .fixed_decimals(2),
                        );
                        if slider_response.double_clicked() {
                            self.settings.decay_rate_per_sec = defaults::DECAY_RATE_PER_SEC;
                        }
                        r1.union(slider_response).on_hover_text_at_pointer("How fast vibration fades after hold delay (rate per second).");

                    });
                }
            });
            // --- End Persistence Controls ---            

            ui.separator();

            ui.heading("Devices");
            if let Some(client) = &self.client {
                for device in client.devices() {
                    let device_name = device.name().to_string();
                    if !self.devices.contains_key(&device_name) {
                        let props = DeviceProps::new(&self.runtime, device.clone(), &self.settings);
                        self.devices.insert(device_name.clone(), props);
                    }
                    let props = self.devices.get_mut(&device_name).unwrap();
                    device_widget(ui, device, props, self.vibration_level, &self.runtime);
                }
            }
        });
        settings_window_widget(
            ctx,
            &mut self.show_settings,
            &mut self.settings,
        );
        ctx.request_repaint();
    }
}

fn settings_window_widget(
    ctx: &egui::Context,
    show_settings: &mut bool,
    settings: &mut Settings,
) {
    Window::new("Settings")
        .open(show_settings)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.checkbox(&mut settings.use_dark_mode, "Use dark mode");
            ui.checkbox(
                &mut settings.start_scanning_on_startup,
                "Start scanning on startup",
            );
            ui.checkbox(
                &mut settings.save_device_settings,
                "Remember device settings",
            );
            let mut current_value = settings.use_polling_rate.load(Ordering::Relaxed);
            if ui.checkbox(&mut current_value, "Use fixed polling rate").changed() {
                settings.use_polling_rate.store(current_value, Ordering::Relaxed);
            }
            ui.checkbox(
                &mut settings.enable_persistence,
                "Enable vibration persistence",
            );
        });
}

struct VibratorProps {
    is_enabled: bool,
    multiplier: f32,
    min: f32,
    max: f32,
}

impl Default for VibratorProps {
    fn default() -> Self {
        Self {
            is_enabled: true,
            multiplier: 1.0,
            min: 0.0,
            max: 1.0,
        }
    }
}

fn device_widget(
    ui: &mut Ui,
    device: Arc<ButtplugClientDevice>,
    props: &mut DeviceProps,
    vibration_level: f32,
    runtime: &Runtime,
) {
    ui.group(|ui| {
        if cfg!(debug_assertions) {
            ui.label(format!("({}) {}", device.index(), device.name()));
        } else {
            ui.label(device.name());
        }

        if let Some(bat) = props.battery_state.get_level() {
            ui.label(format!("Battery: {}%", bat * 100.0));
        }

        let (speed, cutoff) = props.calculate_visual_output(vibration_level);

        ui.horizontal(|ui| {
            let label = if props.is_enabled {
                "Enabled"
            } else {
                "Enable"
            };
            let enable_button = SelectableLabel::new(props.is_enabled, label);
            ui.vertical(|ui| {
                ui.group(|ui| {
                    if ui.add_sized([60.0, 60.0], enable_button).clicked() {
                        props.is_enabled = !props.is_enabled;
                        if !props.is_enabled {
                            runtime.spawn(device.stop());
                        }
                    }
                });
            });
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("{:.2}%", speed * 100.0));
                    if cutoff {
                        ui.visuals_mut().selection.bg_fill = Color32::RED;
                    }
                    if !props.is_enabled {
                        ui.visuals_mut().selection.bg_fill = Color32::GRAY;
                    }
                    ui.add(ProgressBar::new(speed));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Multiplier: ");
                    let slider_response = ui.add(Slider::new(&mut props.multiplier, 0.0..=20.0));
                    if slider_response.double_clicked() {
                        props.multiplier = 1.0;
                    }
                    ui.label("Minimum (cut-off): ");
                    let slider_response = ui.add(Slider::new(&mut props.min, 0.0..=1.0).fixed_decimals(2));
                    if slider_response.double_clicked() {
                        props.min = 0.0;
                    }
                    ui.label("Maximum: ");
                    let slider_response = ui.add(Slider::new(&mut props.max, 0.0..=1.0).fixed_decimals(2));
                    if slider_response.double_clicked() {
                        props.max = 1.0;
                    }
                });
                ui.push_id(format!("vibrators_{}", device.name()), |ui| {
                    ui.collapsing("Vibrators", |ui| {
                        ui.group(|ui| {
                            for (i, vibe) in props.vibrators.iter_mut().enumerate()
                            {
                                vibrator_widget(ui, i, vibe);
                            }
                        });
                    });
                });
                if props.is_enabled {
                    let speed = props.calculate_output(vibration_level);
                    let speed_cmd = VibrateCommand::SpeedVec(
                        props
                            .vibrators
                            .iter()
                            .map(|v| {
                                if v.is_enabled {
                                    (speed * v.multiplier)
                                        .clamp(0.0, v.max)
                                        .min_cutoff(v.min)
                                        as f64
                                } else {
                                    0.0
                                }
                            })
                            .collect(),
                    );
                    runtime.spawn(device.vibrate(&speed_cmd));
                }
            })
        });
    });
}

fn vibrator_widget(ui: &mut Ui, index: usize, vibe: &mut VibratorProps) {
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("Vibe {index}: "));
        let label = if vibe.is_enabled { "Enabled" } else { "Enable" };
        if ui.selectable_label(vibe.is_enabled, label).clicked() {
            vibe.is_enabled = !vibe.is_enabled;
        }

        ui.label("Multiplier: ");
        let slider_response = ui.add(Slider::new(&mut vibe.multiplier, 0.0..=5.0));
        if slider_response.double_clicked() {
            vibe.multiplier = 1.0;
        }
        ui.label("Minimum (cut-off): ");
        let slider_response = ui.add(Slider::new(&mut vibe.min, 0.0..=1.0).fixed_decimals(2));
        if slider_response.double_clicked() {
            vibe.min = 0.0;
        }
        ui.label("Maximum: ");
        let slider_response = ui.add(Slider::new(&mut vibe.max, 0.0..=1.0).fixed_decimals(2));
        if slider_response.double_clicked() {
            vibe.max = 1.0;
        }
    });
}