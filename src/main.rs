use std::{
    fs::File,
    io::{self, BufReader},
    path::{Path, PathBuf},
    time::Duration,
};

use buttplug::{
    client::{ButtplugClient, ButtplugClientError, VibrateCommand},
    connector::{
        ButtplugRemoteClientConnector as RCC,
        ButtplugWebsocketClientTransport as WSCT,
    },
    core::messages::{
        serializer::ButtplugClientJSONSerializer,
        ButtplugCurrentSpecDeviceMessageType as MsgType,
    },
    util::async_manager::block_on,
};
use clap::Parser;
use rodio::{Decoder, OutputStream, Sink, Source};

fn open_decoder(
    path: impl AsRef<Path>,
) -> io::Result<Decoder<BufReader<File>>> {
    Decoder::new(BufReader::new(File::open(path)?))
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

#[derive(Parser)]
struct Opt {
    #[clap(short, long, default_value = "64")]
    chunk_size: usize,
    /// Path to audio file
    file: PathBuf,
}

fn main() {
    let opts = Opt::parse();
    let chunk_size = opts.chunk_size;

    let (_stream, handle) = OutputStream::try_default().unwrap();
    let sink = Sink::try_new(&handle).unwrap();
    sink.pause();

    let file_name = opts.file;
    let audio = open_decoder(file_name).unwrap().buffered();
    // .take_duration(Duration::from_secs(30));
    let (tx, rx) = flume::bounded(0);
    let sample_rate = audio.sample_rate();
    let channels = audio.channels() as usize;
    let dur = Duration::from_secs(1) * chunk_size as u32 / sample_rate;
    eprintln!("dur: {:?}", dur);
    let audio2 = audio.clone().convert_samples::<f32>();
    let audio = audio.periodic_access(dur, move |_| {
        let _ = tx.send(());
    });
    sink.append(audio);

    let remote_connector = RCC::<_, ButtplugClientJSONSerializer>::new(
        WSCT::new_insecure_connector("ws://127.0.0.1:12345"),
    );
    block_on(async {
        let client = ButtplugClient::new("music-vibes");
        if let Err(e) = client.connect(remote_connector).await {
            eprintln!("Couldn't connect to external server: {}", e);
            eprintln!("Launching in-process server");
            client.connect_in_process(None).await?;
        }

        let server_name = client.server_name();
        let server_name = server_name.as_deref().unwrap_or("<unknown>");
        println!("Server name: {}", server_name);

        client.start_scanning().await?;
        println!("started scanning");
        std::thread::sleep(Duration::from_secs(1));
        client.stop_scanning().await?;

        let devices = client.devices();
        let device = if devices.len() == 1 {
            devices[0].clone()
        } else if devices.len() == 0 {
            panic!("No devices detected")
        } else {
            panic!("Only one device for now, plz")
        };
        println!("found device: {}", device.name);

        let vib_count = device
            .allowed_messages
            .get(&MsgType::VibrateCmd)
            .and_then(|x| x.feature_count)
            .expect("no vibrators");
        println!("vibrators: {}", vib_count);

        sink.play();
        for chunk in audio2.chunks(chunk_size * channels) {
            let mut sums = vec![0.0; channels];
            for frame in chunk.chunks_exact(channels as _) {
                for (acc, sample) in sums.iter_mut().zip(frame) {
                    *acc += sample.abs().powi(2) as f64;
                }
            }
            for sum in sums.iter_mut() {
                *sum /= chunk.len() as f64;
                *sum = sum.sqrt().clamp(0.0, 1.0);
            }
            let avg = {
                let len = sums.len() as f64;
                let avg = sums.into_iter().sum::<f64>() / len;
                vec![avg; vib_count as usize]
            };
            device.vibrate(VibrateCommand::SpeedVec(avg)).await?;

            rx.recv().unwrap();
        }
        drop(rx);
        client.stop_all_devices().await?;
        client.disconnect().await?;
        println!("buttplug finished");
        Ok::<_, ButtplugClientError>(())
    })
    .unwrap();
    sink.sleep_until_end();
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
