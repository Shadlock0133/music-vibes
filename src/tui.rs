use std::{collections::VecDeque, sync::Arc, time::Duration};

use audio_capture::win::capture::AudioCapture;
use buttplug::{
    client::{ButtplugClient, ButtplugClientDevice, VibrateCommand},
    core::messages::ButtplugCurrentSpecDeviceMessageType as MsgType,
    util::async_manager::block_on,
};
use clap::Parser;
use parking_lot::Mutex;

use crate::util;

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
pub struct Tui {
    #[clap(short, default_value = "1.0")]
    multiply: f32,
}

pub fn tui(args: Tui) {
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
            let client = util::start_bp_server().await.unwrap();
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
                let speeds = util::calculate_power(&buf, format.channels as _);
                let speeds = if stereo && vib_count == format.channels as u32 {
                    speeds
                        .into_iter()
                        .map(|x| (x * args.multiply).clamp(0.0, 1.0) as f64)
                        .collect()
                } else {
                    let avg =
                        (util::avg(&speeds) * args.multiply).clamp(0.0, 1.0);
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
