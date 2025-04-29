use std::{
    collections::{HashMap, VecDeque},
    iter::from_fn,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};

use audio_capture::win::capture::AudioCapture;
use buttplug::{
    client::{ButtplugClient, ButtplugClientDevice, VibrateCommand},
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
    settings::{defaults, Settings},
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

struct GuiApp {
    runtime: tokio::runtime::Runtime,
    client: ButtplugClient,
    devices: HashMap<u32, DeviceProps>,
    current_sound_power: SharedF32,
    _capture_thread: JoinHandle<()>,
    is_scanning: bool,
    show_settings: bool,
    // persistent settings
    settings: Settings,
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
    fn new(runtime: &Runtime, device: Arc<ButtplugClientDevice>) -> Self {
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
        let vibrators = from_fn(|| Some(VibratorProps::default()))
            .take(vibe_count)
            .collect();
        Self {
            is_enabled: false,
            battery_state: BatteryState::new(runtime, device),
            multiplier: 1.0,
            min: 0.0,
            max: 1.0,
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
    use_custom_polling_rate: Arc<AtomicBool>,
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
        // Determine sleep duration based on settings
        let use_custom = use_custom_polling_rate.load(Ordering::Relaxed);
        let sleep_duration = if use_custom {
            // Ensure polling rate is at least 1ms
            Duration::from_millis(polling_rate_ms.load().max(1.0) as u64)
        } else {
            actual_duration // Use the default duration based on buffer size
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
        let client = runtime
            .block_on(util::start_bp_server(server_addr))
            .unwrap();
        let devices = Default::default();
        let current_sound_power = SharedF32::new(0.0);
        let current_sound_power2 = current_sound_power.clone();

        let settings = ctx.storage.map(Settings::load).unwrap_or_default();
        let low_pass_freq = settings.low_pass_freq.clone();
        let polling_rate_ms = settings.polling_rate_ms.clone();
        let use_custom_polling_rate = settings.use_custom_polling_rate.clone();

        let _capture_thread = std::thread::spawn(move || {
            capture_thread(
                current_sound_power2,
                low_pass_freq,
                polling_rate_ms,
                use_custom_polling_rate,
            )
        });

        let is_scanning = settings.start_scanning_on_startup;
        if is_scanning {
            runtime.spawn(client.start_scanning());
        }

        GuiApp {
            runtime,
            client,
            devices,
            current_sound_power,
            _capture_thread,
            is_scanning,
            show_settings: false,
            settings,
        }
    }
}

impl eframe::App for GuiApp {
    fn save(&mut self, storage: &mut dyn Storage) {
        self.settings.save(storage);
        storage.flush();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let visuals = match self.settings.use_dark_mode {
            true => Visuals::dark(),
            false => Visuals::light(),
        };
        ctx.set_visuals(visuals);
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                let scan_label = if self.is_scanning {
                    "Stop scanning"
                } else {
                    "Start scanning"
                };
                if ui.selectable_label(self.is_scanning, scan_label).clicked() {
                    self.is_scanning = !self.is_scanning;
                    if self.is_scanning {
                        self.runtime.spawn(self.client.start_scanning());
                    } else {
                        self.runtime.spawn(self.client.stop_scanning());
                    }
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
                    self.runtime.spawn(self.client.stop_all_devices());
                    for device in self.devices.values_mut() {
                        device.is_enabled = false;
                    }
                }
            });
            ui.separator();
            let main_mul = self.settings.main_volume.powi(2);
            let sound_power =
                (self.current_sound_power.load() * main_mul).clamp(0.0, 1.0);
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Current volume: {:.2}%",
                    sound_power * 100.0
                ));
                ui.add(ProgressBar::new(sound_power));
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
                    // Reset on double click
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
                    // Reset on double click
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
                let is_custom_polling_enabled = self.settings.use_custom_polling_rate.load(Ordering::Relaxed);
                if is_custom_polling_enabled {
                    let r1 = ui.label("Polling Rate (ms): ");
                    let mut polling_rate = self.settings.polling_rate_ms.load();
                    let slider = Slider::new(&mut polling_rate, 1.0..=500.0).integer();
                    let slider_response = ui.add(slider); // Use ui.add since it's only shown when enabled
                    if slider_response.changed() {
                        self.settings.polling_rate_ms.store(polling_rate);
                    }
                    if slider_response.double_clicked() {
                        // Reset on double click
                        self.settings
                            .polling_rate_ms
                            .store(defaults::POLLING_RATE_MS);
                    }
                    r1.union(slider_response).on_hover_text_at_pointer(
                        "Controls how often audio is analyzed (in milliseconds).\n\
                        Only use this setting if your device is lagging.",
                    );
                }
            });
            ui.separator();

            ui.heading("Devices");
            for device in self.client.devices() {
                let props =
                    self.devices.entry(device.index()).or_insert_with(|| {
                        DeviceProps::new(&self.runtime, device.clone())
                    });
                device_widget(ui, device, props, sound_power, &self.runtime);
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
            // Custom Polling Rate Checkbox
            let mut current_value = settings.use_custom_polling_rate.load(Ordering::Relaxed);
            if ui.checkbox(&mut current_value, "Use custom polling rate").changed() {
                settings.use_custom_polling_rate.store(current_value, Ordering::Relaxed);
            }
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
    sound_power: f32,
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

        let (speed, cutoff) = props.calculate_visual_output(sound_power);

        ui.horizontal(|ui| {
            let label = if props.is_enabled {
                "Enabled"
            } else {
                "Enable"
            };
            let enable_button = SelectableLabel::new(props.is_enabled, label);
            ui.group(|ui| {
                if ui.add_sized([60.0, 60.0], enable_button).clicked() {
                    props.is_enabled = !props.is_enabled;
                    if !props.is_enabled {
                        runtime.spawn(device.stop());
                    }
                }
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
                        props.multiplier = 1.0; // Default DeviceProps multiplier
                    }
                    ui.label("Minimum (cut-off): ");
                    let slider_response = ui.add(Slider::new(&mut props.min, 0.0..=1.0));
                     if slider_response.double_clicked() {
                        props.min = 0.0; // Default DeviceProps min
                    }
                    ui.label("Maximum: ");
                    let slider_response = ui.add(Slider::new(&mut props.max, 0.0..=1.0));
                     if slider_response.double_clicked() {
                        props.max = 1.0; // Default DeviceProps max
                    }
                });
                ui.collapsing("Vibrators", |ui| {
                    ui.group(|ui| {
                        for (i, vibe) in props.vibrators.iter_mut().enumerate()
                        {
                            vibrator_widget(ui, i, vibe);
                        }
                    });
                });
                if props.is_enabled {
                    let speed = props.calculate_output(sound_power);
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
        })
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
            vibe.multiplier = 1.0; // Default VibratorProps multiplier
        }
        ui.label("Minimum (cut-off): ");
        let slider_response = ui.add(Slider::new(&mut vibe.min, 0.0..=1.0));
        if slider_response.double_clicked() {
            vibe.min = 0.0; // Default VibratorProps min
        }
        ui.label("Maximum: ");
        let slider_response = ui.add(Slider::new(&mut vibe.max, 0.0..=1.0));
        if slider_response.double_clicked() {
            vibe.max = 1.0; // Default VibratorProps max
        }

        if ui.button("Reset").clicked() {
            *vibe = VibratorProps::default();
        }
    });
}
