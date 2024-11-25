//! wires up the grpc layer to the rest of the app

use crate::{
    pb::{
        self,
        jobs::{
            store_client::StoreClient, store_server::StoreServer, KillRequest, KillResponse,
            LogFrame, LogsRequest, PsRequest, PsResponse, SpawnRequest, SpawnResponse,
        },
    },
    proc::{
        coord::{Coordinator, FrameEvent},
        types::{Ctx, JobId, JobSpec, User},
    },
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures::Future;
use rustls::crypto::aws_lc_rs;
use std::{net::SocketAddr, pin::Pin};
use tokio::sync::mpsc::Receiver;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{
    async_trait,
    transport::{
        Certificate, CertificateDer, Channel, ClientTlsConfig, Identity, Server, ServerTlsConfig,
        Uri,
    },
    Request, Response, Status,
};
use tracing::{error, info, instrument, warn};

// to configure the lower level details of the tls behavior, including what are passable cipher
// suites, it seems like we need to manually configure rustls before spinning up tonic. it's likely
// that a more robust way of doing this exists within tonic itself, but i had some difficulty
// sussing that out.
//
// i made the choice to use the aws-lc-rs crypto lib over ring because this is the default that
// tonic uses, and i'm not familiar with the tradeoffs between the two.
fn init_crypto(verbose: bool) {
    let provider = rustls::crypto::CryptoProvider {
        // because we control the client and it is a greenfield, we make the choice to avoid tls1.2
        // altogether. Priority is given to the two suites with a larger key size. There is an
        // argument for omitting `TLS13_AES_128_GCM_SHA256` altogether if we have full control over
        // the clients.
        cipher_suites: vec![
            aws_lc_rs::cipher_suite::TLS13_AES_256_GCM_SHA384,
            aws_lc_rs::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
            aws_lc_rs::cipher_suite::TLS13_AES_128_GCM_SHA256, // 🔪❓
        ],
        ..aws_lc_rs::default_provider()
    };
    // only print once on installation of defaults. sometimes there will be multiple ones per
    // process, e.g. during testing.
    if provider.clone().install_default().is_ok() && verbose {
        info!("Initialized rustls/aws-lc-rs crypto provider.");
        info!("Enabled cipher suites in priority order:");
        for suite in &provider.cipher_suites {
            info!("- {suite:?}");
        }
    }
}

pub struct ClientOpts {
    pub endpoint: Uri,
    pub domain: String,
    pub identity: Identity,
    pub server_ca: Certificate,
}

impl ClientOpts {
    /// Creates a new, connected, client which is ready to use.
    pub async fn connect(self) -> Result<StoreClient<Channel>> {
        init_crypto(false);
        let tls = ClientTlsConfig::new()
            .domain_name(self.domain)
            .ca_certificate(self.server_ca)
            .identity(self.identity);
        let channel = Channel::builder(self.endpoint)
            .tls_config(tls)
            .context("invalid tls config")?
            .connect()
            .await
            .context("connect failure")?;
        Ok(StoreClient::new(channel))
    }
}

pub struct ServerOpts<C, S> {
    pub bind: SocketAddr,
    pub identity: Identity,
    pub client_ca: Certificate,
    pub coord: C,
    pub shutdown: S,
}

impl<C, S> ServerOpts<C, S>
where
    C: Coordinator,
    S: Future<Output = ()>,
{
    /// initializes and runs the server.
    pub async fn run_server(self) -> Result<()> {
        init_crypto(true);
        info!("Starting server...");
        let tls_config = ServerTlsConfig::new()
            .identity(self.identity)
            .client_ca_root(self.client_ca)
            .client_auth_optional(false);
        let store_impl = GrpcStoreImpl::new(self.coord);
        let store_server = StoreServer::with_interceptor(store_impl, server_interceptor);
        info!("Listening on {}", self.bind);
        Server::builder()
            .tls_config(tls_config)
            .context("invalid tls config")?
            .add_service(store_server)
            .serve_with_shutdown(self.bind, self.shutdown)
            .await
            .context("server failed")
    }
}

/// the context can be parsed out from the mtls client cert in the request
impl<'a> TryFrom<CertificateDer<'a>> for Ctx {
    type Error = Status;
    fn try_from(cert: CertificateDer<'a>) -> Result<Self, Self::Error> {
        let bs = cert.as_ref().to_vec();
        let (_bs, x509) = x509_parser::parse_x509_certificate(&bs)
            .map_err(|err| Status::unauthenticated(format!("bad cert: {err}")))?;
        let subject = x509.subject().to_string();
        let ctx = Ctx {
            user: User { id: subject },
        };
        Ok(ctx)
    }
}

/// helpers to get/set the client identity on a request. these could have been normal fns, but it
/// felt more natural to be able to ask the request to do these things.
trait RequestExt {
    fn set_ctx(&mut self, ident: Ctx);
    fn get_ctx(&self) -> Result<Ctx, Status>;
}

impl<T> RequestExt for Request<T> {
    fn set_ctx(&mut self, ctx: Ctx) {
        self.extensions_mut().insert(ctx);
    }
    fn get_ctx(&self) -> Result<Ctx, Status> {
        self.extensions()
            .get::<Ctx>()
            .cloned()
            .ok_or_else(|| Status::unauthenticated("no ctx found"))
    }
}

/// applied to every server request
fn server_interceptor(mut req: Request<()>) -> Result<Request<()>, Status> {
    let chain = &[extract_client_cert];
    for i in chain {
        req = i(req)?;
    }
    Ok(req)
}

/// an interceptor that gets the client cert from the request and sets it as an extension on the
/// request
fn extract_client_cert(mut req: Request<()>) -> Result<Request<()>, Status> {
    let certs = req
        .peer_certs()
        .ok_or(Status::unauthenticated("mtls required"))?;
    let cert = match certs.as_slice() {
        [cert] => cert.to_owned(),
        certs => {
            return Err(Status::unauthenticated(format!(
                "expected exactly one peer cert but got {}",
                certs.len()
            )))
        }
    };
    let ident = cert.try_into()?;
    req.set_ctx(ident);
    Ok(req)
}

/// implements the gRPC Store service. responsible for delegating incoming gRPC requests to the
/// backing store, as well as adapting the responses to the gRPC interface.
#[derive(Debug, Clone)]
struct GrpcStoreImpl<C> {
    coord: C,
}

impl<C> GrpcStoreImpl<C> {
    pub fn new(coord: C) -> Self {
        Self { coord }
    }
}

type FrameResult = Result<LogFrame, Status>;
type LogStream = Pin<Box<dyn futures::Stream<Item = FrameResult> + Send>>;

/// map incoming grpc requests to the backing store
#[async_trait]
impl<S> pb::jobs::store_server::Store for GrpcStoreImpl<S>
where
    S: Coordinator,
{
    /// Server streaming response type for the Logs method.
    type LogsStream = LogStream;

    /// spawns a new job
    #[instrument(skip_all)]
    async fn spawn(&self, req: Request<SpawnRequest>) -> Result<Response<SpawnResponse>, Status> {
        let ctx = req.get_ctx()?;
        info!("spawn: {ctx}");
        let req: pb::jobs::SpawnRequest = req.into_inner();
        req.validate()?;
        let cmd = JobSpec::new(&req.name).args(&req.args);
        self.coord
            .spawn(ctx, cmd)
            .await
            .map(|job| pb::jobs::SpawnResponse {
                job: Some(job.into()),
            })
            .map(Response::new)
            .map_err(|err| {
                error!("spawn: {err:#}");
                err.into()
            })
    }

    /// fetches the logs for a particular job
    #[instrument(skip_all)]
    async fn logs(&self, req: Request<LogsRequest>) -> Result<Response<Self::LogsStream>, Status> {
        let ctx = req.get_ctx()?;
        info!("logs: {ctx}");
        let req: LogsRequest = req.into_inner();
        req.validate()?;
        let job_id = JobId::from(req.job_id);
        let mut records: Receiver<FrameEvent> =
            self.coord.logs(ctx, job_id).await.inspect_err(|err| {
                error!("logs: {err:#}");
            })?;
        // i know there is a more fluent way to map futures into other futures without using a task
        // and a while rx/tx loop, but i am not that comfortable with the futures crate just yet.
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tokio::task::spawn(async move {
            loop {
                let Some(event) = records.recv().await else {
                    warn!("log stream terminated unexpectedly");
                    let _ = tx
                        .send(Err(Status::internal("log stream terminated unexpectedly")))
                        .await;
                    break;
                };
                match event {
                    FrameEvent::Frame(frame) => {
                        let frame = LogFrame {
                            kind: frame.kind.to_log_frame_kind(),
                            bs: frame.bs.clone(),
                        };
                        // NB: we just convert this to an Ok, but a future version could map the Err
                        // variant to why the stream stopped suddenly (e.g. the client was killed).
                        // Skipping this for time.
                        if tx.send(Ok(frame)).await.is_err() {
                            warn!("client conn dropped");
                            break;
                        }
                    }
                    FrameEvent::Error(err) => {
                        // the log stream encountered an error.
                        let msg = format!("log stream failed: {err}");
                        let _ = tx.send(Err(Status::internal(msg))).await;
                        break;
                    }
                    FrameEvent::EOF => {
                        // it is expected that a normal log stream will terminate with the EOF
                        // event unless the process crashes.
                        break;
                    }
                }
            }
        });
        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::LogsStream))
    }

    /// queries job state for one of the user's jobs.
    #[instrument(skip_all)]
    async fn ps(&self, req: Request<PsRequest>) -> Result<Response<PsResponse>, Status> {
        let ctx = req.get_ctx()?;
        info!("ps: {ctx}");
        let req: PsRequest = req.into_inner();
        req.validate()?;
        self.coord
            .ps(ctx, req.job_id.into())
            .await
            .map(|job| pb::jobs::PsResponse {
                job: Some(job.into()),
            })
            .map(Response::new)
            .map_err(|err| {
                error!("ps: {err:#}");
                err.into()
            })
    }

    /// terminates a specific job. only the owner of the job is allowed to kill it.
    #[instrument(skip_all)]
    async fn kill(&self, req: Request<KillRequest>) -> Result<Response<KillResponse>, Status> {
        let ctx = req.get_ctx()?;
        info!("kill: {ctx}");
        let req: KillRequest = req.into_inner();
        req.validate()?;
        self.coord
            .kill(ctx, req.job_id.into())
            .await
            .map(|job| pb::jobs::KillResponse {
                job: Some(job.into()),
            })
            .map(Response::new)
            .map_err(|err| {
                error!("kill: {err:#}");
                err.into()
            })
    }
}

/// untrusted incoming data must be validated
trait Validate {
    fn validate(&self) -> Result<(), Status>;
}

impl Validate for SpawnRequest {
    fn validate(&self) -> Result<(), Status> {
        if self.name.is_empty() {
            return Err(Status::invalid_argument("name required"));
        }
        Ok(())
    }
}

impl Validate for LogsRequest {
    fn validate(&self) -> Result<(), Status> {
        validate_job_id(&self.job_id)?;
        Ok(())
    }
}

impl Validate for PsRequest {
    fn validate(&self) -> Result<(), Status> {
        validate_job_id(&self.job_id)?;
        Ok(())
    }
}

impl Validate for KillRequest {
    fn validate(&self) -> Result<(), Status> {
        validate_job_id(&self.job_id)?;
        Ok(())
    }
}

fn validate_job_id(job_id: &str) -> Result<(), Status> {
    if job_id.is_empty() {
        return Err(Status::invalid_argument("job id required"));
    }
    Ok(())
}

// the idea here is that we'll map specific known errors to status messages to avoid things leaking
// unexpectedly.
impl From<crate::proc::coord::Error> for Status {
    fn from(err: crate::proc::coord::Error) -> Self {
        match err {
            crate::proc::coord::Error::JobNotFound { .. }
            | crate::proc::coord::Error::JobNotRunning { .. } => Self::not_found("job not found"),
            // fallback: internal error
            _ => Self::internal(""),
        }
    }
}

impl From<crate::proc::coord::Frame> for pb::jobs::LogFrame {
    fn from(frame: crate::proc::coord::Frame) -> Self {
        pb::jobs::LogFrame {
            kind: frame.kind.to_log_frame_kind(),
            bs: frame.bs,
        }
    }
}

impl crate::proc::coord::StreamKind {
    fn to_log_frame_kind(self) -> i32 {
        match self {
            crate::proc::coord::StreamKind::Stdout => pb::jobs::LogFrameKind::Stdout.into(),
            crate::proc::coord::StreamKind::Stderr => pb::jobs::LogFrameKind::Stderr.into(),
        }
    }
}

impl From<crate::proc::coord::StreamKind> for pb::jobs::LogFrameKind {
    fn from(knd: crate::proc::coord::StreamKind) -> Self {
        match knd {
            crate::proc::coord::StreamKind::Stdout => pb::jobs::LogFrameKind::Stdout,
            crate::proc::coord::StreamKind::Stderr => pb::jobs::LogFrameKind::Stderr,
        }
    }
}

impl From<crate::proc::types::Job> for pb::jobs::Job {
    fn from(job: crate::proc::types::Job) -> Self {
        pb::jobs::Job {
            id: job.id.to_string(),
            owner: Some(job.user.into()),
            started_at: Some(datetime_to_timestamp(job.started_at)),
            ended_at: job.ended_at.map(datetime_to_timestamp),
            killed: job.killed.map(|k| pb::jobs::Killed {
                killed_at: Some(datetime_to_timestamp(k.killed_at)),
                user: Some(k.user.into()),
            }),
            state: Some(match job.state {
                crate::proc::types::JobState::Running { pid } => {
                    pb::jobs::job::State::Running(pb::jobs::JobRunningState { pid: pid.pid })
                }
                crate::proc::types::JobState::Exited { status } => {
                    pb::jobs::job::State::Exited(pb::jobs::JobExitedState {
                        // TODO: figure out what this should be.
                        exit_code: status.code.unwrap(),
                    })
                }
            }),
        }
    }
}

impl From<crate::proc::types::User> for pb::jobs::User {
    fn from(user: crate::proc::types::User) -> Self {
        pb::jobs::User { id: user.id }
    }
}

fn datetime_to_timestamp(dt: DateTime<Utc>) -> prost_types::Timestamp {
    let st = std::time::SystemTime::from(dt);
    prost_types::Timestamp::from(st)
}

// These tests are super bare and minimal, and serve to show how i'd probably setup the grpc
// testing suite. i'd create an impl of the Coordinator that an actual grpc server would delegate
// to. the tests would use mockall to simulate the output of the Coordinator impl, and then the
// tests would verify that the various responses from the Coordinator would map to grpc outputs.
// unfortunately I'm short on time so i've left this somewhat barren.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        grpc,
        proc::{
            coord::{Error as CoordError, FrameEvent},
            types::Job,
        },
        utils,
    };
    use futures::channel::oneshot;
    use std::net::TcpListener;
    use tokio::{sync::mpsc::Receiver, task::JoinSet};
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn spawn_simple() {
        let test = Test::new().await;
        let mut client = test.new_client().await;
        let req = Request::new(SpawnRequest {
            name: String::from("echo"),
            args: vec![],
        });
        let resp = client.spawn(req).await.unwrap().into_inner();
        let job = resp.job.unwrap();
        assert_eq!(job.id, "job-id", "{job:#?}");
        assert_eq!(
            job.owner,
            Some(pb::jobs::User {
                id: String::from("test-user")
            }),
            "{job:#?}"
        );
    }

    #[allow(unused)]
    struct Test {
        shutdown_tx: oneshot::Sender<()>,
        bind: SocketAddr,
        server: JoinSet<Result<(), anyhow::Error>>,
    }

    impl Test {
        /// starts a server in a background task
        async fn new() -> Self {
            let cert = "data/tls/server.pem";
            let key = "data/tls/server.key";
            let client_ca = "data/tls/client_ca.pem";
            let identity = utils::read_identity(cert, key).await.unwrap();
            let client_ca = utils::read_ca_cert(client_ca).await.unwrap();
            let coord = FakeCoord::default();
            // this is a bad way to configure the server but it's a shortcut for now. this lets me
            // know what port might be available. i'll then use the addr to run the server.
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let bind = l.local_addr().unwrap();
            info!("Spinning up test server on {bind}");
            drop(l);
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let shutdown = async move {
                let _ = shutdown_rx.await;
            };
            let opts = grpc::ServerOpts {
                bind,
                identity,
                client_ca,
                coord,
                shutdown,
            };
            let mut server = JoinSet::new();
            server.spawn(opts.run_server());
            Test {
                shutdown_tx,
                bind,
                server,
            }
        }

        /// TODO: parameterize this so that test code can build different clients with different
        /// identities
        async fn new_client(&self) -> StoreClient<Channel> {
            let server_ca = utils::read_ca_cert("data/tls/server_ca.pem").await.unwrap();
            let cert = "data/tls/client1.pem";
            let key = "data/tls/client1.key";
            let identity = utils::read_identity(cert, key).await.unwrap();
            let endpoint = format!("https://{}", self.bind);
            let endpoint = endpoint.parse().unwrap();
            let domain = "localhost".to_string();
            let client_opts = super::ClientOpts {
                endpoint,
                domain,
                identity,
                server_ca,
            };
            client_opts.connect().await.unwrap()
        }
    }

    /// if i had more time, this would probably use a mockall impl for Coordinator.
    #[derive(Default)]
    struct FakeCoord {}

    #[allow(unused)]
    impl crate::proc::coord::Coordinator for FakeCoord {
        /// Spawns a new job according to the supplied job spec.
        async fn spawn(&self, ctx: Ctx, spec: JobSpec) -> Result<Job, CoordError> {
            Ok(Job {
                id: "job-id".into(),
                user: "test-user".into(),
                killed: None,
                started_at: Utc::now(),
                ended_at: None,
                state: crate::proc::types::JobState::Running { pid: 42.into() },
            })
        }

        /// fetches the logs for the specified job
        async fn logs(&self, ctx: Ctx, job_id: JobId) -> Result<Receiver<FrameEvent>, CoordError> {
            todo!("impl logs")
        }

        /// queries the state of the specified job
        async fn ps(&self, ctx: Ctx, job_id: JobId) -> Result<Job, CoordError> {
            todo!("impl ps")
        }

        /// kills the specified job
        async fn kill(&self, ctx: Ctx, job_id: JobId) -> Result<Job, CoordError> {
            todo!("impl kill")
        }
    }
}
