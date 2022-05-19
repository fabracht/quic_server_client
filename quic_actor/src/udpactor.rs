mod handlers;
mod messages;

use actix::io::SinkWrite;
use actix::{Actor, ActorContext, Addr, AsyncContext, Context, StreamHandler};
use bytes::BufMut;
use dashmap::DashMap;
use quiche::ConnectionId;
use ring::hmac::Key;
use ring::rand::SystemRandom;

use std::collections::HashMap;
// use std::io::{BufRead};
use std::net::SocketAddr;

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::stream::SplitSink;
use futures_util::StreamExt;
use log::{debug, error, info, trace, warn};
use tokio::net::UdpSocket;
use tokio_util::codec::{self};
use tokio_util::udp::UdpFramed;

use crate::MAX_DATAGRAM_SIZE;
#[allow(dead_code)]
type UdpSink = SinkWrite<
    (bytes::BytesMut, SocketAddr),
    SplitSink<UdpFramed<codec::BytesCodec>, (String, SocketAddr)>,
>;

struct PartialResponse {
    body: Vec<u8>,

    written: usize,
}
pub struct Client {
    conn: std::pin::Pin<Box<quiche::Connection>>,
    partial_responses: HashMap<u64, PartialResponse>,
}

pub struct UdpServerActor {
    pub sink_write: Arc<UdpSocket>,
    #[allow(dead_code)]
    clients: DashMap<ConnectionId<'static>, Client>,
    #[allow(dead_code)]
    config: quiche::Config,
    #[allow(dead_code)]
    hb: Instant,
    conn_id_seed: Key,
}

#[allow(dead_code)]
impl UdpServerActor {
    pub fn start(udp: UdpSocket, config: quiche::Config) -> Addr<UdpServerActor> {
        let udp_arc = Arc::new(udp);
        let framed = UdpFramed::new(udp_arc.clone(), codec::BytesCodec::default());
        let (_split_sink, split_stream) = framed.split::<(bytes::BytesMut, SocketAddr)>();

        let rng = SystemRandom::new();
        let conn_id_seed = ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap();
        UdpServerActor::create(|ctx| {
            ctx.add_stream(split_stream);
            // let sink_write: UdpSink = SinkWrite::new(udp_arc, ctx);
            UdpServerActor {
                sink_write: udp_arc,
                clients: DashMap::default(),
                config,
                hb: Instant::now(),
                conn_id_seed,
            }
        })
    }

    fn send_to(&self, len: usize, pkt_buf: &mut [u8], socket_address: SocketAddr) -> bool {
        let sink_clone = Arc::clone(&self.sink_write);

        let out = Vec::from(pkt_buf);
        let fut = async move {
            match sink_clone.send_to(&out[..len], socket_address).await {
                Ok(v) => error!("Successfuly sent {} bytes", v),
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        debug!("send() would block");
                    }
                    panic!("send() failed: {:?}", e);
                }
            }
        };
        actix_rt::System::current().arbiter().spawn(fut)
    }

    fn send_new_token(
        &mut self,
        hdr: &quiche::Header,
        socket_address: SocketAddr,
        scid: &ConnectionId,
        pkt_buf: &mut [u8],
    ) -> bool {
        warn!("Doing stateless retry");
        let new_token = mint_token(hdr, &socket_address);
        let len =
            quiche::retry(&hdr.scid, &hdr.dcid, scid, &new_token, hdr.version, pkt_buf).unwrap();
        self.send_to(len, pkt_buf, socket_address)
    }
}

impl Actor for UdpServerActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        // let message = b"Ping";
        let socket = Arc::clone(&self.sink_write);
        let _address = socket.local_addr();

        ctx.run_interval(Duration::from_millis(5000), |a, _c| {
            a.clients.retain(|id, c| {
                // info!("Collecting garbage from {:?}", id);
                if c.conn.is_closed() {
                    error!(
                        "{} connection collected {:?}",
                        c.conn.trace_id(),
                        c.conn.stats()
                    );
                }
                !c.conn.is_closed()
            });
        });
    }
}

impl StreamHandler<Result<(bytes::BytesMut, std::net::SocketAddr), std::io::Error>>
    for UdpServerActor
{
    fn handle(
        &mut self,
        item: Result<(bytes::BytesMut, std::net::SocketAddr), std::io::Error>,
        ctx: &mut Self::Context,
    ) {
        match item {
            Ok((mut message, socket_address)) => {
                let len = message.len();

                let mut pkt_buf = &mut message[..len];
                
                let header_option =
                    match quiche::Header::from_slice(pkt_buf, quiche::MAX_CONN_ID_LEN) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            error!("Parsing packet header failed: {:?}", e);
                            None
                        }
                    };
                info!("Got packet {:?}", header_option);
                /////////////////////////////////////////
                /////////////////////////////////////////
                ///// MATCH QUIC HEADER
                /////////////////////////////////////////
                /////////////////////////////////////////
                if let Some(hdr) = header_option {
                    let conn_id = ring::hmac::sign(&self.conn_id_seed, &hdr.dcid);
                    let conn_id = &conn_id.as_ref()[..quiche::MAX_CONN_ID_LEN];
                    let conn_id = conn_id.to_vec().into();
                    /////////////////////////////////////////
                    /////////////////////////////////////////
                    ///// CLIENT CREATION
                    /////////////////////////////////////////
                    /////////////////////////////////////////
                    let mut client = if !self.clients.contains_key(&hdr.dcid)
                        && !self.clients.contains_key(&conn_id)
                    {
                        warn!("New Client");
                        //////////////////////////////////////////////////////
                        if hdr.ty != quiche::Type::Initial {
                            error!("Packet is not Initial");
                        }

                        if !quiche::version_is_supported(hdr.version) {
                            warn!("Doing version negotiation");

                            let len = quiche::negotiate_version(&hdr.scid, &hdr.dcid, &mut pkt_buf)
                                .unwrap();

                            self.send_to(len, pkt_buf, socket_address);
                            return;
                        }

                        let mut scid = [0; quiche::MAX_CONN_ID_LEN];
                        scid.copy_from_slice(&conn_id);

                        let scid = quiche::ConnectionId::from_ref(&scid);

                        // Token is always present in Initial packets.
                        let token = hdr.token.as_ref().unwrap();

                        // Do stateless retry if the client didn't send a token.
                        if token.is_empty() {
                            self.send_new_token(&hdr, socket_address, &scid, pkt_buf);
                            return;
                        }

                        let odcid = validate_token(&socket_address, token);

                        // The token was not valid, meaning the retry failed, so
                        // drop the packet.
                        if odcid.is_none() {
                            error!("Invalid address validation token");
                            return;
                        }

                        if scid.len() != hdr.dcid.len() {
                            error!("Invalid destination connection ID");
                            return;
                        }

                        // Reuse the source connection ID we sent in the Retry packet,
                        // instead of changing it again.
                        let scid = hdr.dcid.clone();

                        debug!("New connection: dcid={:?} scid={:?}", hdr.dcid, scid);

                        let conn =
                            quiche::accept(&scid, odcid.as_ref(), socket_address, &mut self.config)
                                .unwrap();

                        let client = Client {
                            conn,
                            partial_responses: HashMap::default(),
                        };

                        self.clients.insert(scid.clone(), client);

                        let client = self.clients.get_mut(&scid).unwrap();
                        client
                    } else {
                        match self.clients.get_mut(&hdr.dcid) {
                            Some(v) => {
                                info!("Client found");
                                v
                            }

                            None => {
                                warn!("Client found 2");
                                self.clients.get_mut(&conn_id).unwrap()
                            }
                        }
                    };

                    /////////////////////////////////////////
                    /////////////////////////////////////////
                    ///// READ INCOMING DATA
                    /////////////////////////////////////////
                    /////////////////////////////////////////
                    let recv_info = quiche::RecvInfo {
                        from: socket_address,
                    };

                    let read = match client.conn.recv(pkt_buf, recv_info) {
                        Ok(v) => v,

                        Err(e) => {
                            error!("{} recv failed: {:?}", client.conn.trace_id(), e);
                            return;
                        }
                    };
                    info!("{} processed {} bytes", client.conn.trace_id(), read);
                    if client.conn.is_in_early_data() || client.conn.is_established() {
                        trace!("Connection is Established");

                        // Handle writable streams.
                        for stream_id in client.conn.writable() {
                            handle_writable(&mut client, stream_id);
                        }

                        // Process all readable streams.
                        for s in client.conn.readable() {
                            trace!("Client connection is readable");
                            while let Ok((read, fin)) = client.conn.stream_recv(s, &mut pkt_buf) {
                                warn!("{} received {} bytes", client.conn.trace_id(), read);

                                let stream_buf = &pkt_buf[..read];

                                info!(
                                    "{} stream {} has {} bytes (fin? {})",
                                    client.conn.trace_id(),
                                    s,
                                    stream_buf.len(),
                                    fin
                                );
                                handle_stream(&mut client, s, stream_buf, "./");
                            }
                        }
                    }
                };

                for mut client_ref in self.clients.iter_mut() {
                    warn!("Handling early established");
                    handle_early_established(&mut client_ref, pkt_buf);
                    loop {
                        // let client = client_ref.value_mut();
                        let (write, send_info) = match client_ref.conn.send(&mut pkt_buf) {
                            Ok(v) => v,

                            Err(quiche::Error::Done) => {
                                error!("{} done writing", client_ref.conn.trace_id());
                                break;
                            }

                            Err(e) => {
                                error!("{} send failed: {:?}", client_ref.conn.trace_id(), e);

                                client_ref.conn.close(false, 0x1, b"fail").ok();
                                break;
                            }
                        };

                        if self.send_to(write, pkt_buf, send_info.to) == false {
                            break;
                        } else {
                            warn!("{} written {} bytes", client_ref.conn.trace_id(), write);
                        }

                        // std::thread::sleep(Duration::from_millis(250));
                    }
                }
                //////////////////////////////////////////////////////
            }
            Err(_e) => {
                ctx.stop();
            }
        }
    }
}

fn handle_early_established(
    client_ref: &mut dashmap::mapref::multiple::RefMutMulti<ConnectionId, Client>,
    mut pkt_buf: &mut [u8],
) {
    if client_ref.conn.is_in_early_data() || client_ref.conn.is_established() {
        trace!("Connection is Established");

        // Handle writable streams.
        for stream_id in client_ref.conn.writable() {
            handle_writable(client_ref, stream_id);
        }

        // Process all readable streams.
        for s in client_ref.conn.readable() {
            trace!("Client connection is readable");
            while let Ok((read, fin)) = client_ref.conn.stream_recv(s, &mut pkt_buf) {
                warn!("{} received {} bytes", client_ref.conn.trace_id(), read);

                let stream_buf = &pkt_buf[..read];

                info!(
                    "{} stream {} has {} bytes (fin? {})",
                    client_ref.conn.trace_id(),
                    s,
                    stream_buf.len(),
                    fin
                );
                // let mut bclient = client.value().clone();
                handle_stream(client_ref, s, stream_buf, "./");
            }
        }
    }
}

impl actix::io::WriteHandler<std::io::Error> for UdpServerActor {
    fn finished(&mut self, ctx: &mut Self::Context) {
        error!("Finished");
        ctx.stop();
    }
}

/// Generate a stateless retry token.
///
/// The token includes the static string `"quiche"` followed by the IP address
/// of the client and by the original destination connection ID generated by the
/// client.
///
/// Note that this function is only an example and doesn't do any cryptographic
/// authenticate of the token. *It should not be used in production system*.
fn mint_token(hdr: &quiche::Header, src: &std::net::SocketAddr) -> Vec<u8> {
    let mut token = Vec::new();

    token.extend_from_slice(b"quiche");

    let addr = match src.ip() {
        std::net::IpAddr::V4(a) => a.octets().to_vec(),
        std::net::IpAddr::V6(a) => a.octets().to_vec(),
    };

    token.extend_from_slice(&addr);
    token.extend_from_slice(&hdr.dcid);

    token
}

/// Validates a stateless retry token.
///
/// This checks that the ticket includes the `"quiche"` static string, and that
/// the client IP address matches the address stored in the ticket.
///
/// Note that this function is only an example and doesn't do any cryptographic
/// authenticate of the token. *It should not be used in production system*.
fn validate_token<'a>(
    src: &std::net::SocketAddr,
    token: &'a [u8],
) -> Option<quiche::ConnectionId<'a>> {
    if token.len() < 6 {
        return None;
    }

    if &token[..6] != b"quiche" {
        return None;
    }

    let token = &token[6..];

    let addr = match src.ip() {
        std::net::IpAddr::V4(a) => a.octets().to_vec(),
        std::net::IpAddr::V6(a) => a.octets().to_vec(),
    };

    if token.len() < addr.len() || &token[..addr.len()] != addr.as_slice() {
        return None;
    }

    Some(quiche::ConnectionId::from_ref(&token[addr.len()..]))
}

/// Handles incoming HTTP/0.9 requests.
fn handle_stream(client: &mut Client, stream_id: u64, buf: &[u8], root: &str) {
    let conn = &mut client.conn;

    if buf.len() > 4 && &buf[..4] == b"GET " {
        let uri = &buf[4..buf.len()];
        let uri = String::from_utf8(uri.to_vec()).unwrap();
        let uri = String::from(uri.lines().next().unwrap());
        let uri = std::path::Path::new(&uri);
        let mut path = std::path::PathBuf::from(root);

        for c in uri.components() {
            if let std::path::Component::Normal(v) = c {
                path.push(v)
            }
        }

        info!(
            "{} got GET request for {:?} on stream {}",
            conn.trace_id(),
            path,
            stream_id
        );
        let file_name = path.as_path().file_name().unwrap();
        let body = match std::fs::read(file_name) {
            Ok(value) => value,
            Err(_e) => {
                error!("File not found in path {:?}", file_name);
                b"Not Found!\r\n".to_vec()
            }
        };
        // let file = std::fs::File::open(file_name).unwrap();
        // let buf_read = BufReader::new(file);

        // let body = buf_read.buffer();
        error!(
            "{} sending response of size {} on stream {}",
            conn.trace_id(),
            body.len(),
            stream_id
        );
        let written = match conn.stream_send(stream_id, &body, true) {
            Ok(v) => v,

            Err(quiche::Error::Done) => 0,

            Err(e) => {
                error!("{} stream send failed {:?}", conn.trace_id(), e);
                return;
            }
        };

        if written < body.len() {
            let response = PartialResponse {
                body: body.to_vec(),
                written,
            };
            client.partial_responses.insert(stream_id, response);
        }
    }
}

/// Handles newly writable streams.
fn handle_writable(client: &mut Client, stream_id: u64) -> usize {
    let conn = &mut client.conn;

    warn!("{} stream {} is writable", conn.trace_id(), stream_id);

    if !client.partial_responses.contains_key(&stream_id) {
        return 0;
    }

    let resp = client.partial_responses.get_mut(&stream_id).unwrap();
    let body = &resp.body[resp.written..];
    warn!("There's still {} bytes to write", body.len());
    // info!("Unsafe stream is {}", unsafe {
    //     std::str::from_utf8_unchecked(body)
    // });
    let written = match conn.stream_send(stream_id, body, false) {
        Ok(v) => v,

        Err(quiche::Error::Done) => 0,

        Err(e) => {
            client.partial_responses.remove(&stream_id);

            error!("{} stream send failed {:?}", conn.trace_id(), e);
            return 0;
        }
    };
    warn!("Wrote {} bytes", written);

    resp.written += written;

    if resp.written == resp.body.len() {
        client.partial_responses.remove(&stream_id);
    }
    written
}
