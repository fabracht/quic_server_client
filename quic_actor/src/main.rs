use std::error::Error;

use actix::Addr;
use actix_rt::net::UdpSocket;
use log::info;
use ring::rand::SystemRandom;

use crate::udpactor::UdpClientActor;

mod udpactor;

fn main() -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
    ////////////////////////////////////////////////////////////////////////////
    std::env::set_var("RUST_LOG", "info");
    std::env::set_var("RUST_LOG_STYLE", "info");
    ////////////////////////////////////////////////////////////////////////////
    ////////////////////////////////////////////////////////////////////////////
    /////////// LOG4RS
    log4rs::init_file("./logging_config.yaml", Default::default()).unwrap();
    let system = actix_rt::System::new();
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;

    config.load_cert_chain_from_pem_file("./cert.pem")?;
    config.load_priv_key_from_pem_file("./key.pem")?;
    // config.set_application_protos(b"\x0ahq-interop\x05hq-29\x05hq-28\x05hq-27\x08http/0.9")?;
    config.set_application_protos(quiche::h3::APPLICATION_PROTOCOL)?;
    config.verify_peer(false);
    config.enable_early_data();
    let _rng = SystemRandom::new();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Addr<UdpClientActor>>(1024);

    let fut = async move {
        let socket = UdpSocket::bind("127.0.0.1:4480").await.unwrap();
        socket.set_broadcast(true).unwrap();
        info!("listening on {:}", socket.local_addr().unwrap());
        let udp_actor_address = UdpClientActor::start(socket, config);
        let _ = tx.send(udp_actor_address).await;
    };
    actix_rt::Arbiter::new().spawn(fut);
    let _udp_actor_address = rx.blocking_recv().unwrap();
    info!("Got actor address!");
    let _ = system.run();
    Ok(())
}
