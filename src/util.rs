use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

use buttplug::{
    client::{ButtplugClient, ButtplugClientError},
    connector::{
        ButtplugRemoteClientConnector as RemoteConn,
        ButtplugWebsocketClientTransport as WebsocketTransport,
    },
    core::messages::serializer::ButtplugClientJSONSerializer as JsonSer,
};

pub async fn start_bp_server() -> Result<ButtplugClient, ButtplugClientError> {
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

#[derive(Clone)]
pub struct SharedF32(Arc<AtomicU32>);

impl SharedF32 {
    pub fn new(v: f32) -> Self {
        Self(Arc::new(AtomicU32::new(v.to_bits())))
    }

    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }

    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

pub fn low_pass(
    samples: &[f32],
    time: Duration,
    rc: f32,
    channels: usize,
) -> Vec<f32> {
    let len = samples.len();
    if len < channels {
        return vec![];
    }
    let mut res = vec![0.0; len];
    let dt = time.as_secs_f32();
    let a = dt / (rc + dt);
    for c in 0..channels {
        res[c] = a * samples[c];
    }
    for i in channels..len {
        res[i] = a * samples[i] + (1.0 - a) * res[i - channels];
    }
    res
}

pub fn calculate_power(samples: &[f32], channels: usize) -> Vec<f32> {
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

pub fn avg(samples: &[f32]) -> f32 {
    let len = samples.len();
    samples.iter().sum::<f32>() / len as f32
}

pub trait MinCutoff {
    fn min_cutoff(self, min: Self) -> Self;
}

impl MinCutoff for f32 {
    fn min_cutoff(self, min: Self) -> Self {
        if self < min {
            0.0
        } else {
            self
        }
    }
}
