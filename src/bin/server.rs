use anyhow::Result;
use clap::Parser;
use jobs::{grpc, proc::coord, utils};
use std::net::SocketAddr;

#[derive(Debug, clap::Parser)]
struct Args {
    /// the server cert
    #[arg(long, default_value = "data/tls/server.pem")]
    cert: String,

    /// the server key
    #[arg(long, default_value = "data/tls/server.key")]
    key: String,

    /// the client CA cert
    #[arg(long, default_value = "data/tls/client_ca.pem")]
    client_ca: String,

    /// the addr to bind to
    #[arg(long, default_value = "0.0.0.0:50051")]
    bind: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    utils::init_tracing();
    let Args {
        cert,
        key,
        client_ca,
        bind,
    } = Args::parse();
    let identity = utils::read_identity(&cert, &key).await?;
    let client_ca = utils::read_ca_cert(&client_ca).await?;
    let coord = coord::MemoryCoordinator::default();
    let shutdown = futures::future::pending::<()>();
    let server_opts = grpc::ServerOpts {
        bind,
        identity,
        client_ca,
        coord,
        shutdown,
    };
    server_opts.run_server().await
}
