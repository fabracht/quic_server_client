use std::error::Error;

use actix::Addr;
use actix_rt::net::UdpSocket;
use log::info;
use ring::rand::SystemRandom;

use crate::udpactor::UdpServerActor;

mod udpactor;
const MAX_DATAGRAM_SIZE: usize = 1350;
fn main() -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
    ////////////////////////////////////////////////////////////////////////////
    std::env::set_var("RUST_LOG", "trace");
    std::env::set_var("RUST_LOG_STYLE", "trace");
    ////////////////////////////////////////////////////////////////////////////
    ////////////////////////////////////////////////////////////////////////////
    /////////// LOG4RS
    log4rs::init_file("./logging_config.yaml", Default::default()).unwrap();
    let system = actix_rt::System::new();
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;

    config.load_cert_chain_from_pem_file("./cert.pem")?;
    config.load_priv_key_from_pem_file("./key.pem")?;
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    // config.set_application_protos(b"\x0ahq-interop\x05hq-29\x05hq-28\x05hq-27\x08http/0.9")?;
    config.set_application_protos(quiche::h3::APPLICATION_PROTOCOL)?;
    config.set_max_idle_timeout(1000);
    config.verify_peer(false);
    config.set_initial_max_data(10_000_000);
    config.set_initial_max_stream_data_bidi_local(1_000_000);
    config.set_initial_max_stream_data_bidi_remote(1_000_000);
    config.set_initial_max_stream_data_uni(10_000_000);
    config.set_initial_max_streams_bidi(1000);
    config.set_initial_max_streams_uni(1000);
    config.set_disable_active_migration(true);
    config.enable_early_data();
    config.enable_dgram(true, 1000, 1000);
    let _rng = SystemRandom::new();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Addr<UdpServerActor>>(1024);

    let fut = async move {
        let socket = UdpSocket::bind("127.0.0.1:4480").await.unwrap();
        socket.set_broadcast(true).unwrap();
        info!("listening on {:}", socket.local_addr().unwrap());
        let udp_actor_address = UdpServerActor::start(socket, config);
        let _ = tx.send(udp_actor_address).await;
    };
    actix_rt::Arbiter::new().spawn(fut);
    let _udp_actor_address = rx.blocking_recv().unwrap();
    info!("Got actor address!");
    let _ = system.run();
    Ok(())
}
