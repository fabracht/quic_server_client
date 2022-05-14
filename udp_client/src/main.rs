// // Copyright (C) 2018-2019, Cloudflare, Inc.
// // All rights reserved.
// //
// // Redistribution and use in source and binary forms, with or without
// // modification, are permitted provided that the following conditions are
// // met:
// //
// //     * Redistributions of source code must retain the above copyright notice,
// //       this list of conditions and the following disclaimer.
// //
// //     * Redistributions in binary form must reproduce the above copyright
// //       notice, this list of conditions and the following disclaimer in the
// //       documentation and/or other materials provided with the distribution.
// //
// // THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
// // IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO,
// // THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
// // PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR
// // CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
// // EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
// // PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
// // PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
// // LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
// // NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
// // SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

// #[macro_use]
// extern crate log;

// use std::net::ToSocketAddrs;

// use mio::net::SocketAddr;
// use ring::rand::*;

// const MAX_DATAGRAM_SIZE: usize = 1350;

// const HTTP_REQ_STREAM_ID: u64 = 4;

// fn main() {
//     let mut buf = [0; 65535];
//     let mut out = [0; MAX_DATAGRAM_SIZE];

//     let mut args = std::env::args();

//     let cmd = &args.next().unwrap();

//     if args.len() != 1 {
//         println!("Usage: {} URL", cmd);
//         println!("\nSee tools/apps/ for more complete implementations.");
//         return;
//     }

//     let url = url::Url::parse("localhost:8080").unwrap();

//     // Setup the event loop.
//     let mut poll = mio::Poll::new().unwrap();
//     let mut events = mio::Events::with_capacity(1024);

//     // Resolve server address.

//     // Bind to INADDR_ANY or IN6ADDR_ANY depending on the IP family of the
//     // server address. This is needed on macOS and BSD variants that don't
//     // support binding to IN6ADDR_ANY for both v4 and v6.
//     let peer_addr = "127.0.0.1:4480".parse().unwrap();
//     let local_addr = "127.0.0.1:8080".parse().unwrap();

//     // Create the UDP socket backing the QUIC connection, and register it with
//     // the event loop.
//     let mut socket =
//         mio::net::UdpSocket::bind(local_addr).unwrap();
//     poll.registry()
//         .register(&mut socket, mio::Token(0), mio::Interest::READABLE)
//         .unwrap();

//     // Create the configuration for the QUIC connection.
//     let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();

//     // *CAUTION*: this should not be set to `false` in production!!!
//     config.verify_peer(false);

//     config
//         .set_application_protos(
//             b"\x0ahq-interop\x05hq-29\x05hq-28\x05hq-27\x08http/0.9",
//         )
//         .unwrap();

//     config.set_max_idle_timeout(5000);
//     config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
//     config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
//     config.set_initial_max_data(10_000_000);
//     config.set_initial_max_stream_data_bidi_local(1_000_000);
//     config.set_initial_max_stream_data_bidi_remote(1_000_000);
//     config.set_initial_max_streams_bidi(100);
//     config.set_initial_max_streams_uni(100);
//     config.set_disable_active_migration(true);
//     // config.set_application_protos(quiche::h3::APPLICATION_PROTOCOL).unwrap();
//     // Generate a random source connection ID for the connection.
//     let mut scid = [0; quiche::MAX_CONN_ID_LEN];
//     SystemRandom::new().fill(&mut scid[..]).unwrap();

//     let scid = quiche::ConnectionId::from_ref(&scid);

//     // Create a QUIC connection and initiate handshake.
//     let mut conn =
//         quiche::connect(url.domain(), &scid, peer_addr, &mut config).unwrap();

//     info!(
//         "connecting to {:} from {:} with scid {}",
//         peer_addr,
//         socket.local_addr().unwrap(),
//         hex_dump(&scid)
//     );

//     let (write, send_info) = conn.send(&mut out).expect("initial send failed");

//     while let Err(e) = socket.send_to(&out[..write], send_info.to) {
//         if e.kind() == std::io::ErrorKind::WouldBlock {
//             debug!("send() would block");
//             continue;
//         }

//         panic!("send() failed: {:?}", e);
//     }

//     debug!("written {}", write);

//     let req_start = std::time::Instant::now();

//     let mut req_sent = false;

//     loop {
//         poll.poll(&mut events, conn.timeout()).unwrap();

//         // Read incoming UDP packets from the socket and feed them to quiche,
//         // until there are no more packets to read.
//         'read: loop {
//             // If the event loop reported no events, it means that the timeout
//             // has expired, so handle it without attempting to read packets. We
//             // will then proceed with the send loop.
//             if events.is_empty() {
//                 debug!("timed out");

//                 conn.on_timeout();
//                 break 'read;
//             }

//             let (len, from) = match socket.recv_from(&mut buf) {
//                 Ok(v) => v,

//                 Err(e) => {
//                     // There are no more UDP packets to read, so end the read
//                     // loop.
//                     if e.kind() == std::io::ErrorKind::WouldBlock {
//                         debug!("recv() would block");
//                         break 'read;
//                     }

//                     panic!("recv() failed: {:?}", e);
//                 },
//             };

//             debug!("got {} bytes", len);

//             let recv_info = quiche::RecvInfo { from };

//             // Process potentially coalesced packets.
//             let read = match conn.recv(&mut buf[..len], recv_info) {
//                 Ok(v) => v,

//                 Err(e) => {
//                     error!("recv failed: {:?}", e);
//                     continue 'read;
//                 },
//             };

//             debug!("processed {} bytes", read);
//         }

//         debug!("done reading");

//         if conn.is_closed() {
//             info!("connection closed, {:?}", conn.stats());
//             break;
//         }

//         // Send an HTTP request as soon as the connection is established.
//         if conn.is_established() && !req_sent {
//             info!("sending HTTP request for {}", url.path());

//             let req = format!("GET {}\r\n", url.path());
//             match conn.stream_send(HTTP_REQ_STREAM_ID, req.as_bytes(), false) {
//                             Ok(v) => {
//                                 req_sent = true;
//                                 info!("{} bytes sent in stream", v)
//                             },
//                             Err(e) => info!("Error: {:?}", e),
//                         }
//             // conn.stream_send(HTTP_REQ_STREAM_ID, req.as_bytes(), true)
//             //     .unwrap();

//             // req_sent = true;
//         }

//         // Process all readable streams.
//         for s in conn.readable() {
//             while let Ok((read, fin)) = conn.stream_recv(s, &mut buf) {
//                 debug!("received {} bytes", read);

//                 let stream_buf = &buf[..read];

//                 debug!(
//                     "stream {} has {} bytes (fin? {})",
//                     s,
//                     stream_buf.len(),
//                     fin
//                 );

//                 print!("{}", unsafe {
//                     std::str::from_utf8_unchecked(stream_buf)
//                 });

//                 // The server reported that it has no more data to send, which
//                 // we got the full response. Close the connection.
//                 if s == HTTP_REQ_STREAM_ID && fin {
//                     info!(
//                         "response received in {:?}, closing...",
//                         req_start.elapsed()
//                     );

//                     conn.close(true, 0x00, b"kthxbye").unwrap();
//                 }
//             }
//         }

//         // Generate outgoing QUIC packets and send them on the UDP socket, until
//         // quiche reports that there are no more packets to be sent.
//         loop {
//             let (write, send_info) = match conn.send(&mut out) {
//                 Ok(v) => v,

//                 Err(quiche::Error::Done) => {
//                     debug!("done writing");
//                     break;
//                 },

//                 Err(e) => {
//                     error!("send failed: {:?}", e);

//                     conn.close(false, 0x1, b"fail").ok();
//                     break;
//                 },
//             };

//             if let Err(e) = socket.send_to(&out[..write], send_info.to) {
//                 if e.kind() == std::io::ErrorKind::WouldBlock {
//                     debug!("send() would block");
//                     break;
//                 }

//                 panic!("send() failed: {:?}", e);
//             }

//             debug!("written {}", write);
//         }

//         if conn.is_closed() {
//             info!("connection closed, {:?}", conn.stats());
//             break;
//         }
//     }
// }

// fn hex_dump(buf: &[u8]) -> String {
//     let vec: Vec<String> = buf.iter().map(|b| format!("{:02x}", b)).collect();

//     vec.join("")
// }

////////////////////////////////////////////////////////
////////////////////////////////////////////////////////
////////////////////////////////////////////////////////
////////////////////////////////////////////////////////

use log::{debug, error, info, warn};
use quiche::{h3::Header, Connection};
use ring::rand::{SecureRandom, SystemRandom};
use std::{io, net::SocketAddr, pin::Pin, time::Duration};
use tokio::net::UdpSocket;
const MAX_DATAGRAM_SIZE: usize = 1350;
const HTTP_REQ_STREAM_ID: u64 = 4;
pub const USER_AGENT: &[u8] = b"quiche-http3-integration-client";

#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    config.verify_peer(false);
    // config
    //     .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
    //     .unwrap();
    config
        .set_application_protos(b"\x0ahq-interop\x05hq-29\x05hq-28\x05hq-27\x08http/0.9")
        .unwrap();

    config.set_max_idle_timeout(5000);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(10_000_000);
    config.set_initial_max_stream_data_uni(10_000_000);
    config.set_initial_max_stream_data_bidi_local(1_000_000);
    config.set_initial_max_stream_data_bidi_remote(1_000_000);
    config.set_initial_max_streams_bidi(1000);
    config.set_initial_max_streams_uni(1000);
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
    loop {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
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
    let p = peer_address.ip().to_string();
    #[allow(unused_assignments)]
    if conn.is_established() && !req_sent {
        info!("sending HTTP request for {}", recv_info.from);
        let mut req = format!("GET {}/README.md\r\n", recv_info.from).into_bytes();

        let mut buf = vec![0; MAX_DATAGRAM_SIZE];
        req.append(&mut buf);
        
        match conn.stream_send(HTTP_REQ_STREAM_ID, &req, true) {
            Ok(v) => {
                req_sent = true;
                info!("{} bytes sent in stream", v)
            }
            Err(e) => error!("Error: {:?}", e),
        }
        // For now, we're not using yet.
    }
    for s in conn.readable() {
        warn!("RUNNING");

        while let Ok((read, fin)) = conn.stream_recv(s, buf) {
            warn!("received {} bytes", read);

            let stream_buf = &buf[..read];

            info!("stream {} has {} bytes (fin? {})", s, stream_buf.len(), fin);

            info!("Unsafe stream is {}", unsafe {
                std::str::from_utf8_unchecked(stream_buf)
            });

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
