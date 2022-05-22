use std::error::Error;

use actix::Addr;
use actix_rt::net::UdpSocket;
use log::info;
use ring::rand::SystemRandom;

use crate::udpactor::UdpServerActor;

mod udpactor;
const MAX_DATAGRAM_SIZE: usize = 1350;
const MAX_UDP_PAYLOAD: usize = 65527;
const MAX_DATA: usize = 5242880;
const MAX_STREAM_DATA: usize = 1048576;
fn main() -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
    ////////////////////////////////////////////////////////////////////////////
    std::env::set_var("RUST_LOG", "trace");
    std::env::set_var("QLOGDIR", "qlog");
    ////////////////////////////////////////////////////////////////////////////
    ////////////////////////////////////////////////////////////////////////////
    /////////// LOG4RS
    log4rs::init_file("./logging_config_server.yaml", Default::default()).unwrap();
    let system = actix_rt::System::new();
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
    config.load_cert_chain_from_pem_file("./cert.pem")?;
    config.load_priv_key_from_pem_file("./key.pem")?;

    config.verify_peer(false);
    // config.set_application_protos(b"\x0ahq-interop\x05hq-29\x05hq-28\x05hq-27\x08http/0.9")?;
    config.set_application_protos(quiche::h3::APPLICATION_PROTOCOL)?;

    config.set_max_recv_udp_payload_size(MAX_UDP_PAYLOAD);
    config.set_max_send_udp_payload_size(MAX_UDP_PAYLOAD);
    config.set_max_idle_timeout(10000);
    config.set_initial_max_data(MAX_DATA as u64);
    config.set_initial_max_stream_data_uni(MAX_STREAM_DATA as u64);
    config.set_initial_max_stream_data_bidi_local(MAX_STREAM_DATA as u64);
    config.set_initial_max_stream_data_bidi_remote(MAX_STREAM_DATA as u64);
    config.set_initial_max_streams_bidi(2);
    config.set_initial_max_streams_uni(2);
    config.set_disable_active_migration(false);

    config.enable_early_data();
    config.enable_dgram(true, 100, 100);
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
