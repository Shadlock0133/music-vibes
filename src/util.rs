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
