use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};

use audio_capture::win::capture::AudioCapture;
use buttplug::{
    client::{
        ButtplugClient, ButtplugClientDevice, ButtplugClientError,
        VibrateCommand,
    },
    connector::{
        ButtplugRemoteClientConnector as RemoteConn,
        ButtplugWebsocketClientTransport as WebsocketTransport,
    },
    core::messages::{
        serializer::ButtplugClientJSONSerializer as JsonSer,
        ButtplugCurrentSpecDeviceMessageType as MsgType,
    },
    util::async_manager::block_on,
};
use clap::Parser;
use eframe::{
    egui::{self, Button, Color32, ProgressBar, RichText, Slider},
    epi,
};
use parking_lot::Mutex;

async fn start_bp_server() -> Result<ButtplugClient, ButtplugClientError> {
    let remote_connector = RemoteConn::<_, JsonSer>::new(
        WebsocketTransport::new_insecure_connector("ws://127.0.0.1:12345"),
    );
    let client = ButtplugClient::new("music-vibes");
    // Fallback to in-process server
    if let Err(e) = client.connect(remote_connector).await {
        eprintln!("Couldn't connect to external server: {}", e);
        eprintln!("Launching in-process server");
        client.connect_in_process(None).await?;
    }

    let server_name = client.server_name();
    let server_name = server_name.as_deref().unwrap_or("<unknown>");
    eprintln!("Server name: {}", server_name);

    Ok(client)
}

#[derive(Debug)]
enum GetDeviceError {
    ZeroDevices,
    MoreThanOneDevice,
}

fn get_device(
    client: &ButtplugClient,
) -> Result<Arc<ButtplugClientDevice>, GetDeviceError> {
    // TODO: handle more than 1 device
    let devices = client.devices();
    let device = if devices.len() == 1 {
        devices[0].clone()
    } else if devices.len() == 0 {
        return Err(GetDeviceError::ZeroDevices);
    } else {
        return Err(GetDeviceError::MoreThanOneDevice);
    };
    Ok(device)
}

#[derive(Parser)]
struct Opt {
    #[clap(subcommand)]
    sub: Option<Subcommand>,
}

#[derive(Parser)]
enum Subcommand {
    /// Listens to system audio
    Tui(Tui),
    /// Launches gui
    Gui(Gui),
}

fn main() {
    let opt = Opt::parse();
    match opt.sub {
        Some(Subcommand::Tui(args)) => tui(args),
        Some(Subcommand::Gui(args)) => gui(args),
        None => gui(Default::default()),
    }
}

#[derive(Parser)]
struct Tui {
    #[clap(short, default_value = "1.0")]
    multiply: f32,
}

fn tui(args: Tui) {
    let stereo = false;
    let dur = Duration::from_millis(1);
    let mut capture = AudioCapture::init(dur).unwrap();

    let format = capture.format().unwrap();
    // time to fill about half of AudioCapture's buffer
    let actual_duration = Duration::from_secs_f32(
        dur.as_secs_f32() * capture.buffer_frame_size as f32
            / format.sample_rate as f32
            / 1000.,
    ) / 2;

    let buffer_size = (format.sample_rate as f32 * dur.as_secs_f32()) as usize
        * format.channels as usize;
    let mut deque = VecDeque::new();
    deque.resize(buffer_size, 0.0);

    let buffer = Arc::new(Mutex::new(deque));
    let buffer2 = buffer.clone();
    let _t = std::thread::spawn(move || {
        block_on(async move {
            let client = start_bp_server().await.unwrap();
            client.start_scanning().await.unwrap();
            let device = get_device(&client).unwrap();
            eprintln!("found device: {}", device.name);

            let vib_count = device
                .allowed_messages
                .get(&MsgType::VibrateCmd)
                .and_then(|x| x.feature_count)
                .expect("no vibrators");
            eprintln!("vibrators: {}", vib_count);
            device.vibrate(VibrateCommand::Speed(1.0)).await.unwrap();

            loop {
                std::thread::sleep(dur);
                let mut buf = buffer.lock();
                let buf = buf.make_contiguous();
                let speeds = calculate_power(&buf, format.channels as _);
                let speeds = if stereo && vib_count == format.channels as u32 {
                    speeds
                        .into_iter()
                        .map(|x| (x * args.multiply).clamp(0.0, 1.0) as f64)
                        .collect()
                } else {
                    let avg = (avg(&speeds) * args.multiply).clamp(0.0, 1.0);
                    vec![avg as _; vib_count as _]
                };
                let res =
                    device.vibrate(VibrateCommand::SpeedVec(speeds)).await;
                if let Err(e) = res {
                    eprintln!("{}", e);
                    break;
                }
            }

            client.stop_all_devices().await.unwrap();
            client.disconnect().await.unwrap();
        });
    });

    capture.start().unwrap();
    loop {
        std::thread::sleep(actual_duration);
        capture
            .read_samples::<(), _>(|samples, _| {
                let mut buf = buffer2.lock();
                for value in samples {
                    buf.push_front(*value);
                }
                buf.truncate(buffer_size);
                Ok(())
            })
            .unwrap();
    }
}

#[derive(Parser, Default)]
struct Gui {
    server_addr: Option<String>,
}

struct GuiApp {
    runtime: tokio::runtime::Runtime,
    client: ButtplugClient,
    devices: HashMap<u32, DeviceProps>,
    current_sound_power: Arc<AtomicU32>,
    capture_thread: Option<JoinHandle<()>>,
    is_scanning: bool,
}

struct DeviceProps {
    is_enabled: bool,
    multiplier: f32,
    max: f32,
}

impl Default for DeviceProps {
    fn default() -> Self {
        Self {
            is_enabled: false,
            multiplier: 1.0,
            max: 1.0,
        }
    }
}

impl GuiApp {
    fn new() -> Self {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = runtime.block_on(start_bp_server()).unwrap();
        let devices = Default::default();
        let current_sound_power = Arc::new(AtomicU32::new(0));
        let current_sound_power2 = current_sound_power.clone();

        let capture_thread = std::thread::spawn(move || {
            let dur = Duration::from_millis(1);
            let mut capture = AudioCapture::init(dur).unwrap();

            let format = capture.format().unwrap();
            // time to fill about half of AudioCapture's buffer
            let actual_duration = Duration::from_secs_f32(
                dur.as_secs_f32() * capture.buffer_frame_size as f32
                    / format.sample_rate as f32
                    / 1000.,
            ) / 2;

            let buffer_size = (format.sample_rate as f32 * dur.as_secs_f32())
                as usize
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
                let speeds = calculate_power(&buf, format.channels as _);
                let avg = avg(&speeds).clamp(0.0, 1.0);
                current_sound_power2.store(avg.to_bits(), Ordering::Relaxed);
            }
        });

        GuiApp {
            runtime,
            client,
            devices,
            current_sound_power,
            capture_thread: Some(capture_thread),
            is_scanning: false,
        }
    }

    fn load_sound_power(&self) -> f32 {
        f32::from_bits(self.current_sound_power.load(Ordering::Relaxed))
    }
}

impl epi::App for GuiApp {
    fn name(&self) -> &str {
        "Music Vibes"
    }

    fn update(&mut self, ctx: &egui::CtxRef, _frame: &epi::Frame) {
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
                let stop_button = Button::new(
                    RichText::new("Stop all devices").color(Color32::BLACK),
                )
                .fill(Color32::from_rgb(240, 0, 0));
                if ui.add_sized([60.0, 30.0], stop_button).clicked() {
                    self.runtime.spawn(self.client.stop_all_devices());
                    for device in self.devices.values_mut() {
                        device.is_enabled = false;
                    }
                }
            });
            ui.separator();
            let sound_power = self.load_sound_power();
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Current volume: {:.2}%",
                    sound_power * 100.0
                ));
                ui.add(ProgressBar::new(sound_power));
            });
            ui.heading("Devices");
            for device in self.client.devices() {
                let props = self.devices.entry(device.index()).or_default();
                ui.group(|ui| {
                    #[cfg(debug_assertions)]
                    ui.label(format!("({}) {}", device.index(), device.name));
                    #[cfg(not(debug_assertions))]
                    ui.label(&device.name);
                    if let Ok(bat) =
                        self.runtime.block_on(device.battery_level())
                    {
                        ui.label(format!("Battery: {}", bat));
                    }
                    let speed = if props.is_enabled {
                        (sound_power * props.multiplier).clamp(0.0, props.max)
                    } else {
                        0.0
                    };

                    ui.horizontal(|ui| {
                        ui.label(format!("{:.2}%", speed * 100.0));
                        ui.add(ProgressBar::new(speed));
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(props.is_enabled, "Enable")
                            .clicked()
                        {
                            props.is_enabled = !props.is_enabled;
                        }
                        ui.label("Multiplier: ");
                        ui.add(Slider::new(&mut props.multiplier, 0.0..=20.0));
                        ui.label("Maximum: ");
                        ui.add(Slider::new(&mut props.max, 0.0..=1.0));
                    });
                    self.runtime.spawn(
                        device.vibrate(VibrateCommand::Speed(speed as _)),
                    );
                });
            }
        });
        ctx.request_repaint();
    }

    fn on_exit(&mut self) {
        drop(self.capture_thread.take());
    }
}

fn gui(_args: Gui) {
    let app = GuiApp::new();
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(Box::new(app), native_options);
}

fn calculate_power(samples: &[f32], channels: usize) -> Vec<f32> {
    let mut sums = vec![0.0; channels];
    for frame in samples.chunks_exact(channels) {
        for (acc, sample) in sums.iter_mut().zip(frame) {
            *acc += sample.abs().powi(2);
        }
    }
    for sum in sums.iter_mut() {
        *sum /= samples.len() as f32;
        *sum = sum.sqrt().clamp(0.0, 1.0);
    }
    sums
}

fn avg(samples: &[f32]) -> f32 {
    let len = samples.len();
    samples.iter().sum::<f32>() / len as f32
}
