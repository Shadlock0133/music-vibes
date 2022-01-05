use std::{
    collections::VecDeque,
    sync::Arc,
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

    client.start_scanning().await?;
    eprintln!("started scanning");
    std::thread::sleep(Duration::from_secs(1));
    client.stop_scanning().await?;

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
enum Opt {
    /// Listens to system audio
    Listen(Listen),
}

fn main() {
    match Opt::parse() {
        Opt::Listen(args) => listen(args),
    }
}

#[derive(Parser)]
struct Listen {
    #[clap(short, default_value = "1.0")]
    multiply: f32,
}

fn listen(args: Listen) {
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

trait IterChunksExt: Iterator + Sized {
    fn chunks(self, size: usize) -> IterChunks<Self> {
        IterChunks(self, size)
    }
}

impl<I> IterChunksExt for I where I: Iterator {}

struct IterChunks<I: Iterator>(I, usize);

impl<I> Iterator for IterChunks<I>
where
    I: Iterator,
{
    type Item = Vec<I::Item>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut res = Vec::with_capacity(self.1);
        for _ in 0..self.1 {
            res.push(self.0.next()?);
        }
        Some(res)
    }
}
