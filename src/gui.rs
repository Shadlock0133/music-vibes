use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    thread::JoinHandle,
    time::Duration,
};

use audio_capture::win::capture::AudioCapture;
use buttplug::client::{ButtplugClient, ButtplugClientDevice, VibrateCommand};
use clap::Parser;
use eframe::{
    egui::{
        self, Button, Color32, ProgressBar, RichText, Slider, Ui, Visuals,
        Window,
    },
    CreationContext, Storage,
};
use tokio::runtime::Runtime;

use crate::{
    settings::Settings,
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
    // volatile info
    is_scanning: bool,
    show_settings: bool,
    // persistent settings
    settings: Settings,
}

struct DeviceProps {
    is_enabled: bool,
    multiplier: f32,
    min: f32,
    max: f32,
}

impl Default for DeviceProps {
    fn default() -> Self {
        Self {
            is_enabled: false,
            multiplier: 1.0,
            min: 0.0,
            max: 1.0,
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

fn capture_thread(sound_power: SharedF32, low_pass_freq: SharedF32) -> ! {
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
        std::thread::sleep(actual_duration);
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

        let _capture_thread = std::thread::spawn(|| {
            capture_thread(current_sound_power2, low_pass_freq)
        });

        GuiApp {
            runtime,
            client,
            devices,
            current_sound_power,
            _capture_thread,
            is_scanning: false,
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
            let sound_power = self.current_sound_power.load();
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Current volume: {:.2}%",
                    sound_power * 100.0
                ));
                ui.add(ProgressBar::new(sound_power));
            });

            ui.horizontal(|ui| {
                let mut low_pass_freq = self.settings.low_pass_freq.load();
                ui.label("Low pass freq.: ");
                ui.add(
                    Slider::new(&mut low_pass_freq, 0.0..=20_000.0)
                        .logarithmic(true)
                        .integer(),
                );
                self.settings.low_pass_freq.store(low_pass_freq);
            });
            ui.separator();

            ui.heading("Devices");
            for device in self.client.devices() {
                let props = self.devices.entry(device.index()).or_default();
                device_widget(ui, device, props, sound_power, &self.runtime);
            }
        });
        Window::new("Settings")
            .open(&mut self.show_settings)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.checkbox(&mut self.settings.use_dark_mode, "Use dark mode");
            });
        ctx.request_repaint();
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
            ui.label(format!("({}) {}", device.index(), device.name));
        } else {
            ui.label(&device.name);
        }
        if let Ok(bat) = runtime.block_on(device.battery_level()) {
            ui.label(format!("Battery: {}", bat));
        }
        let (speed, cutoff) = props.calculate_visual_output(sound_power);

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
            if ui.selectable_label(props.is_enabled, "Enable").clicked() {
                props.is_enabled = !props.is_enabled;
                if !props.is_enabled {
                    runtime.spawn(device.stop());
                }
            }
            ui.label("Multiplier: ");
            ui.add(Slider::new(&mut props.multiplier, 0.0..=20.0));
            ui.label("Minimum (cut-off): ");
            ui.add(Slider::new(&mut props.min, 0.0..=1.0));
            ui.label("Maximum: ");
            ui.add(Slider::new(&mut props.max, 0.0..=1.0));
        });
        if props.is_enabled {
            let speed = props.calculate_output(sound_power) as f64;
            runtime.spawn(device.vibrate(VibrateCommand::Speed(speed)));
        }
    });
}
