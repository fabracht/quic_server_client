mod handlers;
mod messages;

use actix::io::SinkWrite;
use actix::{Actor, ActorContext, Addr, AsyncContext, Context, StreamHandler};
use dashmap::DashMap;
use quiche::ConnectionId;
use ring::hmac::Key;
use ring::rand::SystemRandom;
use std::net::SocketAddr;

use std::sync::Arc;
use std::time::Instant;

use futures_util::stream::SplitSink;
use futures_util::StreamExt;
use log::{debug, error, info, warn};
use tokio::net::UdpSocket;
use tokio_util::codec::{self};
use tokio_util::udp::UdpFramed;
#[allow(dead_code)]
type UdpSink = SinkWrite<
    (bytes::BytesMut, SocketAddr),
    SplitSink<UdpFramed<codec::BytesCodec>, (String, SocketAddr)>,
>;

pub struct Client {
    pub conn: std::pin::Pin<Box<quiche::Connection>>,
}

pub struct UdpClientActor {
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
impl UdpClientActor {
    pub fn start(udp: UdpSocket, config: quiche::Config) -> Addr<UdpClientActor> {
        let udp_arc = Arc::new(udp);
        let framed = UdpFramed::new(udp_arc.clone(), codec::BytesCodec::default());
        let (_split_sink, split_stream) = framed.split::<(bytes::BytesMut, SocketAddr)>();

        let rng = SystemRandom::new();
        let conn_id_seed = ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap();
        UdpClientActor::create(|ctx| {
            ctx.add_stream(split_stream);
            // let sink_write: UdpSink = SinkWrite::new(udp_arc, ctx);
            UdpClientActor {
                sink_write: udp_arc,
                clients: DashMap::default(),
                config,
                hb: Instant::now(),
                conn_id_seed,
            }
        })
    }

    fn send_to(&mut self, len: usize, pkt_buf: &mut [u8], socket_address: SocketAddr) {
        let sink_clone = Arc::clone(&self.sink_write);

        let out = Vec::from(pkt_buf);
        let fut = async move {
            match sink_clone.send_to(&out[..len], socket_address).await {
                Ok(v) => info!("Successfuly sent {} bytes", v),
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        debug!("send() would block");
                    }
                    panic!("send() failed: {:?}", e);
                }
            }
        };
        actix_rt::System::current().arbiter().spawn(fut);
    }
}

impl Actor for UdpClientActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        let message = b"Ping";
        let socket = Arc::clone(&self.sink_write);
        let _address = socket.local_addr();

        let _fut = async move {
            match socket.send(message).await {
                Ok(_) => warn!("Message Sent"),
                Err(e) => warn!("Error sending message {:?}", e),
            }
        };
        // fut.into_actor(self).map(|_a, _b, _c| {}).wait(ctx);
        // futures::executor::block_on(async move {
        // socket.clone().send(message).into_actor(self).map(|r, a, c| {}).wait(ctx);
        // });
        // ctx.run_interval(Duration::from_millis(1000), move |act, inner_ctx| {
        //     let socket = act.sink_write.clone();

        //     if Instant::now().duration_since(act.hb) >= Duration::from_millis(2000) {
        //         actix_rt::spawn(async move {

        //             socket.send(message).await.unwrap();
        //         });
        //     }
        // });
    }
}

impl StreamHandler<Result<(bytes::BytesMut, std::net::SocketAddr), std::io::Error>>
    for UdpClientActor
{
    fn handle(
        &mut self,
        item: Result<(bytes::BytesMut, std::net::SocketAddr), std::io::Error>,
        ctx: &mut Self::Context,
    ) {
        match item {
            Ok((mut message, socket_address)) => {
                let len = message.len();

                // bytes.resize(MAX_BUF_SIZE, 0u8);
                let mut pkt_buf = &mut message[..len];
                // self.clients.insert(Uuid::new_v4(), v.1);
                let header_option = match quiche::Header::from_slice(pkt_buf, len) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        error!("Parsing packet header failed: {:?}", e);
                        None
                    }
                };
                info!("Got packet {:?}", header_option);
                /////////////////////////////////////////
                /////////////////////////////////////////

                /////////////////////////////////////////
                /////////////////////////////////////////
                if let Some(hdr) = header_option {
                    let conn_id = ring::hmac::sign(&self.conn_id_seed, &hdr.dcid);
                    let conn_id = &conn_id.as_ref()[..quiche::MAX_CONN_ID_LEN];
                    let conn_id = conn_id.to_vec().into();

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
                        }

                        let mut scid = [0; quiche::MAX_CONN_ID_LEN];
                        scid.copy_from_slice(&conn_id);

                        let scid = quiche::ConnectionId::from_ref(&scid);

                        // Token is always present in Initial packets.
                        let token = hdr.token.as_ref().unwrap();

                        // Do stateless retry if the client didn't send a token.
                        if token.is_empty() {
                            warn!("Doing stateless retry");

                            let new_token = mint_token(&hdr, &socket_address);

                            let len = quiche::retry(
                                &hdr.scid,
                                &hdr.dcid,
                                &scid,
                                &new_token,
                                hdr.version,
                                &mut pkt_buf,
                            )
                            .unwrap();

                            self.send_to(len, pkt_buf, socket_address);
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

                        info!("New connection: dcid={:?} scid={:?}", hdr.dcid, scid);

                        let conn =
                            quiche::accept(&scid, odcid.as_ref(), socket_address, &mut self.config)
                                .unwrap();

                        let client = Client { conn };

                        self.clients.insert(scid.clone(), client);

                        self.clients.get_mut(&scid).unwrap()
                    } else {
                        match self.clients.get_mut(&hdr.dcid) {
                            Some(v) => {
                                warn!("Client found 1");
                                v
                            }

                            None => {
                                warn!("Client found 2");
                                self.clients.get_mut(&conn_id).unwrap()
                            }
                        }
                    };
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
                    // fut.into_actor(self).map(|a, b, c| {}).wait(ctx);
                    //////////////////////////////////////////////////////
                };
            }
            Err(_e) => {
                ctx.stop();
            }
        }
    }
}

impl actix::io::WriteHandler<std::io::Error> for UdpClientActor {
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
