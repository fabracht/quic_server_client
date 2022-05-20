use log::{debug, error, info, warn};
use quiche::{Config, Connection};
use ring::rand::{SecureRandom, SystemRandom};
use std::{
    io::{self, BufWriter, Write},
    net::SocketAddr,
    pin::Pin,
    time::Duration,
};
use tokio::{net::UdpSocket, time::MissedTickBehavior};

const MAX_DATAGRAM_SIZE: usize = 65527;
const HTTP_REQ_STREAM_ID: u64 = 128;
pub const USER_AGENT: &[u8] = b"quiche-http3-integration-client";
pub struct Client {
    socket: UdpSocket,
    connection: Pin<Box<Connection>>,
    peer_address: SocketAddr,
    req_sent: bool,
    rx: futures::channel::oneshot::Receiver<ClientMessage>,
}

pub enum ClientMessage {
    Shutdown,
}

impl Client {
    pub fn new(
        config: &mut Config,
        peer_address: SocketAddr,
        udp_socket: UdpSocket,
        rx: futures::channel::oneshot::Receiver<ClientMessage>,
    ) -> Self {
        let mut scid = [0; quiche::MAX_CONN_ID_LEN];
        SystemRandom::new().fill(&mut scid[..]).unwrap();
        let scid = quiche::ConnectionId::from_ref(&scid);
        let conn = quiche::connect(Some("quic.tech"), &scid, peer_address, config).unwrap();

        Self {
            socket: udp_socket,
            connection: conn,
            peer_address,
            req_sent: false,
            rx,
        }
    }

    pub async fn run(&mut self, interval: &mut tokio::time::Interval) {
        let mut out = [0; MAX_DATAGRAM_SIZE];

        loop {
            let mut buf = [0; MAX_DATAGRAM_SIZE];

            tokio::select! {
                res = self.socket.recv(&mut buf) => {
                    if let Ok(value) = res {
                        self.handle_response(value, &mut buf).await;
                    }
                }
                _time = interval.tick() => {
                    let (write, send_info) = match self.connection.send(&mut out) {
                        Ok(v) => {
                            v
                        },

                        Err(quiche::Error::Done) => {
                            debug!("done writing");
                            continue;
                        },

                        Err(e) => {
                            error!("send failed: {:?}", e);

                            self.connection.close(false, 0x1, b"fail").ok();
                            break;
                        },
                    };
                    info!("Sending {}, with info: {:?}", write, send_info);
                    let _e = self.socket.send_to(&out[..write], &send_info.to).await;
                }
                _message = &mut self.rx => {
                    break;
                }
            }
        }
    }

    async fn handle_response(&mut self, value: usize, buf: &mut [u8]) {
        // self.connection.on_timeout();
        if self.connection.is_closed() {
            info!("connection closed, {:?}", self.connection.stats());
            return;
        }
        let recv_info = quiche::RecvInfo {
            from: self.peer_address,
        };

        let t = self.connection.recv(&mut buf[..value], recv_info).unwrap();
        info!("{:?} bytes received from {:?}", t, recv_info.from);
        let _p = self.peer_address.ip().to_string();
        #[allow(unused_assignments)]
        if self.connection.is_established() && !self.req_sent {
            error!("sending HTTP request for {}", recv_info.from);
            let mut req = format!("GET {}/LONG_README.md\r\n", recv_info.from).into_bytes();

            let mut buf = vec![0; MAX_DATAGRAM_SIZE];
            req.append(&mut buf);

            match self.connection.stream_send(HTTP_REQ_STREAM_ID, &req, true) {
                Ok(v) => {
                    self.req_sent = true;
                    info!("{} bytes sent in stream", v)
                }
                Err(e) => error!("Error: {:?}", e),
            }
            // self.req_sent = true;
            // For now, we're not using yet.
        }
        for s in self.connection.readable() {
            error!("STREAM_ID = {}", s);
            let mut output = BufWriter::new(
                std::fs::OpenOptions::new()
                    .append(true)
                    .open("./output.txt")
                    .expect("Failed opening file"),
            );

            while let Ok((read, fin)) = self.connection.stream_recv(s, buf) {
                warn!("received {} bytes", read);

                let stream_buf = &buf[..read];

                info!("stream {} has {} bytes (fin? {})", s, stream_buf.len(), fin);

                info!("Unsafe stream is {}", unsafe {
                    std::str::from_utf8_unchecked(stream_buf)
                });

                let written = output.write(stream_buf).unwrap();
                warn!("Wrote {} bytes to file", written);
                // The server reported that it has no more data to send, which
                // we got the full response. Close the connection.
                warn!("STREAM ID: {} is FIN? {}", s, fin);
                if s == HTTP_REQ_STREAM_ID && fin {
                    warn!(
                        "response received in, closing...",
                        // req_start.elapsed()
                    );

                    self.connection.close(true, 0x00, b"kthxbye").unwrap();
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    log4rs::init_file("./logging_config.yaml", Default::default()).unwrap();

    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    config.load_cert_chain_from_pem_file("./cert.pem").unwrap();
    config.load_priv_key_from_pem_file("./key.pem").unwrap();

    config.verify_peer(false);
    // config
    //     .set_application_protos(b"\x0ahq-interop\x05hq-29\x05hq-28\x05hq-27\x08http/0.9")
    //     .unwrap();
    config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();

    // config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    // config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_idle_timeout(10000);
    config.set_initial_max_data(MAX_DATAGRAM_SIZE as u64);
    config.set_initial_max_stream_data_uni(MAX_DATAGRAM_SIZE as u64);
    config.set_initial_max_stream_data_bidi_local(MAX_DATAGRAM_SIZE as u64);
    config.set_initial_max_stream_data_bidi_remote(MAX_DATAGRAM_SIZE as u64);

    config.set_initial_max_streams_bidi(100);
    config.set_initial_max_streams_uni(100);

    config.set_disable_active_migration(false);
    config.enable_dgram(true, 100, 100);

    let sock = UdpSocket::bind("0.0.0.0:8080").await?;
    let peer_address = SocketAddr::new([127, 0, 0, 1].into(), 4480);
    // let peer_address = SocketAddr::new([172, 67, 9, 235].into(), 443);

    let (_tx, rx) = futures::channel::oneshot::channel::<ClientMessage>();

    let mut connection_client = Client::new(&mut config, peer_address, sock, rx);
    let mut interval = tokio::time::interval(Duration::from_nanos(1));
    // let _result = tx.send(ClientMessage::Shutdown);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    connection_client.run(&mut interval).await;
    Ok(())
}
