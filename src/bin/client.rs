use anyhow::{Context, Result};
use clap::Parser;
use jobs::{
    grpc::{self},
    pb::jobs::{
        JobCrashedState, JobExitedState, JobRunningState, KillRequest, LogFrameKind, LogsRequest,
        PsRequest, SpawnRequest,
    },
    utils,
};
use tokio::io::AsyncWriteExt;
use tonic::{transport::Uri, Request};

#[derive(Debug, clap::Parser)]
struct Args {
    /// the client cert
    #[arg(long, default_value = "data/tls/client1.pem")]
    cert: String,

    /// the client key
    #[arg(long, default_value = "data/tls/client1.key")]
    key: String,

    /// the server CA cert
    #[arg(long, default_value = "data/tls/server_ca.pem")]
    server_ca: String,

    /// the domain against which the server cert is verified. defaults to the domain of the
    /// endpoint.
    #[arg(long)]
    domain: Option<String>,

    /// the endpoint of the server
    #[arg(long, default_value = "https://localhost:50051")]
    endpoint: Uri,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    Run { cmd: String, args: Vec<String> },
    Logs { job_id: String },
    Ps { job_id: String },
    Kill { job_id: String },
}

/// unpacks an ok grpc response, otherwise exit (code = 1)
fn must_ok<T>(res: Result<T, tonic::Status>) -> T {
    match res {
        Ok(v) => v,
        Err(status) => {
            let code = status.code();
            let msg = status.message();
            tracing::error!("server: [code:{code:?}] {msg:#?}");
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    utils::init_tracing();
    let Args {
        cert,
        key,
        server_ca,
        domain,
        endpoint,
        command,
    } = Args::parse();
    let domain = domain
        .or_else(|| endpoint.host().map(ToString::to_string))
        .context("domain could not be inferred from endpoint")?;
    let server_ca = utils::read_ca_cert(&server_ca).await?;
    let identity = utils::read_identity(&cert, &key).await?;
    let client_opts = grpc::ClientOpts {
        endpoint,
        domain,
        identity,
        server_ca,
    };
    let mut client = client_opts.connect().await.context("connect client")?;
    match command {
        Command::Run { cmd, args } => {
            let req = Request::new(SpawnRequest { name: cmd, args });
            let resp = must_ok(client.spawn(req).await).into_inner();
            let job = resp.job.context("no job in response")?;
            println!("{}", job.id);
        }
        Command::Logs { job_id } => {
            let req = Request::new(LogsRequest { job_id });
            let mut stream = must_ok(client.logs(req).await).into_inner();
            while let Some(frame) = must_ok(stream.message().await) {
                match frame.kind() {
                    LogFrameKind::Stdout => {
                        let mut out = tokio::io::stdout();
                        out.write_all(&frame.bs).await.context("write to stdout")?;
                        out.flush().await.context("flush stdout")?;
                    }
                    LogFrameKind::Stderr => {
                        let mut out = tokio::io::stderr();
                        out.write_all(&frame.bs).await.context("write to stderr")?;
                        out.flush().await.context("flush stderr")?;
                    }
                }
            }
        }
        Command::Ps { job_id } => {
            let req = Request::new(PsRequest { job_id });
            let resp = must_ok(client.ps(req).await).into_inner();
            let job = resp.job.context("no job in response")?;
            print_job(job)?;
        }
        Command::Kill { job_id } => {
            let req = Request::new(KillRequest { job_id });
            let resp = must_ok(client.kill(req).await).into_inner();
            let job = resp.job.context("no job in response")?;
            print_job(job)?;
        }
    };
    Ok(())
}

fn print_job(job: jobs::pb::jobs::Job) -> Result<()> {
    use jobs::pb::jobs::job::State;
    let state = job.state.context("no job state in reponse")?;
    match state {
        State::Running(JobRunningState { pid }) => {
            println!("Running pid={pid}");
        }
        State::Exited(JobExitedState { exit_code }) => {
            println!("Exited code={exit_code}");
        }
        // NB: i will probably roll up crashed into exited.
        // i don't think it makes sense the way it is now.
        State::Crashed(JobCrashedState { signal, msg }) => {
            println!("Crashed signal={signal} msg={msg}")
        }
    };
    Ok(())
}
