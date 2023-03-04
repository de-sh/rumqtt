use std::time::Duration;

use rumqttc::{AsyncClient, Key, MqttOptions, QoS, TlsConfiguration, Transport};
use tokio::{task, time};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = MqttOptions::new("test-1", "0.0.0.0", 12345);
    let tls_config = TlsConfiguration::Simple {
        ca: include_bytes!("/home/devdutt/bytebeam/rumqtt/certs/ca.cert.pem").to_vec(),
        client_auth: Some((
            include_bytes!("/home/devdutt/bytebeam/rumqtt/certs/12345.cert.pem").to_vec(),
            Key::RSA(include_bytes!("/home/devdutt/bytebeam/rumqtt/certs/12345.key.pem").to_vec()),
        )),
        alpn: None,
    };
    options.set_transport(Transport::Quic(tls_config, 6789, "localhost".to_string()));

    let (client, mut eventloop) = AsyncClient::new(options, 10);

    task::spawn(async move {
        requests(client).await;
        time::sleep(Duration::from_secs(3)).await;
    });

    loop {
        let event = eventloop.poll().await;
        match &event {
            Ok(v) => {
                println!("Event = {v:?}");
            }
            Err(e) => {
                println!("Error = {e:?}");
                return Ok(());
            }
        }
    }
}

async fn requests(client: AsyncClient) {
    client
        .subscribe("hello/world", QoS::AtMostOnce)
        .await
        .unwrap();

    for i in 1..=10 {
        client
            .publish("hello/world", QoS::ExactlyOnce, false, vec![1; i])
            .await
            .unwrap();

        time::sleep(Duration::from_secs(1)).await;
    }

    time::sleep(Duration::from_secs(120)).await;
}
