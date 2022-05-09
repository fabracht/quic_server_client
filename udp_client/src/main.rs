use log::{debug, error, info};
use quiche::Connection;
use ring::rand::{SecureRandom, SystemRandom};
use std::{io, net::SocketAddr, pin::Pin, time::Duration};
use tokio::net::UdpSocket;
const MAX_DATAGRAM_SIZE: usize = 1350;
const HTTP_REQ_STREAM_ID: u64 = 4;

#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    config.verify_peer(false);
    config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();
    config.set_max_idle_timeout(5000);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(10_000_000);
    config.set_initial_max_stream_data_bidi_local(1_000_000);
    config.set_initial_max_stream_data_bidi_remote(1_000_000);
    config.set_initial_max_streams_bidi(100);
    config.set_initial_max_streams_uni(100);
    config.set_disable_active_migration(true);

    let sock = UdpSocket::bind("0.0.0.0:8080").await?;
    let mut out = [0; MAX_DATAGRAM_SIZE];
    let peer_address = SocketAddr::new([127, 0, 0, 1].into(), 4480);
    // 172.67.9.235
    // let peer_address = SocketAddr::new([172, 67, 9, 235].into(), 443);
    let mut buf = [0; MAX_DATAGRAM_SIZE];
    config.load_cert_chain_from_pem_file("./cert.pem").unwrap();
    config.load_priv_key_from_pem_file("./key.pem").unwrap();

    let mut scid = [0; quiche::MAX_CONN_ID_LEN];
    SystemRandom::new().fill(&mut scid[..]).unwrap();
    let scid = quiche::ConnectionId::from_ref(&scid);

    let mut conn = quiche::connect(Some("quic.tech"), &scid, peer_address, &mut config).unwrap();

    let mut interval = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            res = sock.recv(&mut buf) => {
                if let Ok(value) = res {
                    // sock.connect(peer_address).await?;
                    handle_response(value, peer_address.clone(), &mut conn, &mut buf.clone()).await;
                }
            }
            _time = interval.tick() => {
                let (write, send_info) = match conn.send(&mut out) {
                    Ok(v) => v,

                    Err(quiche::Error::Done) => {
                        debug!("done writing");
                        continue;
                    },

                    Err(e) => {
                        error!("send failed: {:?}", e);

                        conn.close(false, 0x1, b"fail").ok();
                        continue;
                    },
                };
                info!("Sent {}, with info: {:?}", write, send_info);
                let _e = sock.send_to(&out[..write], &send_info.to).await;
                info!("Connection is established {:?}", conn.is_established());
            }
        }
    }
    // Ok(())
}

async fn handle_response(
    value: usize,
    peer_address: SocketAddr,
    conn: &mut Pin<Box<Connection>>,
    buf: &mut [u8],
) {
    conn.on_timeout();
    if conn.is_closed() {
        info!("connection closed, {:?}", conn.stats());
        return;
    }
    let mut req_sent = false;
    let recv_info = quiche::RecvInfo { from: peer_address };
    info!("Received info {:?}", recv_info);

    let t = conn.recv(&mut buf[..value], recv_info).unwrap();
    info!("{:?} bytes received from {:?}", t, recv_info.from);
    
    #[allow(unused_assignments)]
    if conn.is_established() && !req_sent {
        info!("sending HTTP request for {}", recv_info.from);

        let req = format!("GET {}\r\n", recv_info.from);
        conn.stream_send(HTTP_REQ_STREAM_ID, req.as_bytes(), true)
            .unwrap();
        // For now, we're not using yet.
        req_sent = true;
    }

    for s in conn.readable() {
        while let Ok((read, fin)) = conn.stream_recv(s, buf) {
            info!("received {} bytes", read);

            let stream_buf = &buf[..read];

            info!("stream {} has {} bytes (fin? {})", s, stream_buf.len(), fin);

            print!("{}", unsafe { std::str::from_utf8_unchecked(stream_buf) });

            // The server reported that it has no more data to send, which
            // we got the full response. Close the connection.
            if s == HTTP_REQ_STREAM_ID && fin {
                info!(
                    "response received in, closing...",
                    // req_start.elapsed()
                );

                conn.close(true, 0x00, b"kthxbye").unwrap();
            }
        }
    }
}
