# Attempted QUIC server client using Quiche crate
This repo contains 2 workspaces. One for the quic server and one for the client.
The server => quic_actor folder
The client => udp_client folder

After cloning the repo, in the project folder on your favorite CLI, type:
## To run the server
```bash
cargo run --bin quic-actor
```
***In a new terminal***
## To run the client
```bash
cargo run --bin udp-client
```
