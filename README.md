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

# What this project is about
This project is my try at writing a client/server application using QUIC. Currently, the application does the following:

* The server starts listening on localhost:4480
* The client starts and sends a "GET" (not really a get, more like a string with the GET word in it) with a filename in the PATH of the request (currently [LONG_README.md](./LONG_README.md))
* The server reads the file and sends it back to the client
* The client saves the content in to a local file named [output.txt](./output.txt)
    * The current behavior is to append to the file, so don't forget to cleanup between runs


# Current problems
Buffer sizes seem to be important for a proper communication. Right now, only a part of the file is saved. Playing with the buffer values will produce different file sizes.
I'm trying to find a solution to this problem. If you have any, don't hesitate to contact me, or, preferably, answer the issue [here](https://github.com/cloudflare/quiche/issues/1230)