//! The main coordinator module. Coordinates jobs. The gprc layer communicates with this through
//! the MemoryCoordinator type which implements Coordinator. Job spawning and related operations
//! are done here.

use super::types::{Ctx, Job, JobId, JobSpec, User};
use crate::proc::{
    spawn,
    types::{JobKilled, JobState, Pid},
};
use chrono::Utc;
use core::fmt;
use futures::Future;
use std::{
    collections::{HashMap, VecDeque},
    io::{self},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::{
        mpsc::{self, error::SendError, Receiver, Sender},
        oneshot, Mutex,
    },
    time::error::Elapsed,
};
use tracing::{debug, error, info, instrument, warn};

type Result<T, E = Error> = std::result::Result<T, E>;

/// The main error export from this module.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("job {job_id} was not found")]
    JobNotFound { job_id: JobId },

    #[error("spawned child process for job {job_id} had no pid")]
    NoPid { job_id: JobId },

    #[error("could not get logs for job: {err}")]
    LogsNotAvailable {
        #[source]
        err: FrameError,
    },

    #[error("specific job was not running")]
    JobNotRunning { job_id: JobId },

    #[error("failed to kill job: {0}")]
    Kill(#[source] KillError),

    #[error(transparent)]
    Spawn(spawn::Error),
}

fn not_found(job_id: &JobId) -> Error {
    Error::JobNotFound {
        job_id: job_id.clone(),
    }
}

fn logs_unavail(err: FrameError) -> Error {
    Error::LogsNotAvailable { err }
}

fn job_not_running(job_id: &JobId) -> Error {
    Error::JobNotRunning {
        job_id: job_id.clone(),
    }
}

/// Handles job lifecycle. The APIs are similar to those in the gRPC service, but more typical of
/// normal rust code. The transport layers will do light translation between its APIs and those of
/// the coordinator. Similarly, the types it works with are similar but distinct due to the
/// flexibility of gRPC values, like optional messages, being invalid at this layer of the app.
/// Clients of the coordinator will need to perform their own conversion to the coordinator types.
///
/// The trait will allow us to test our transport layer more easily.
pub trait Coordinator: Send + Sync + 'static {
    /// Spawns a new job according to the supplied job spec.
    fn spawn(&self, ctx: Ctx, spec: JobSpec) -> impl Future<Output = Result<Job>> + Send;

    /// fetches the logs for the specified job
    fn logs(
        &self,
        ctx: Ctx,
        job_id: JobId,
    ) -> impl Future<Output = Result<Receiver<FrameEvent>>> + Send;

    /// queries the state of the specified job
    fn ps(&self, ctx: Ctx, job_id: JobId) -> impl Future<Output = Result<Job>> + Send;

    /// kills the specified job
    fn kill(&self, ctx: Ctx, job_id: JobId) -> impl Future<Output = Result<Job>> + Send;
}

/// A Coordinator that manages its state in memory.
#[derive(Clone, Default)]
pub struct MemoryCoordinator {
    inner: SharedMemory,
}

/// the internals of the memory coord can be shared
type SharedMemory = Arc<Mutex<Memory>>;

/// tracks jobs. jobs that are successfully started end up in the jobs field. there is currently no
/// mechanism to prune jobs, but that would be trivial to add. after a job finishes its logs still
/// need to be available, and this is why the trackers stay resident.
#[derive(Default)]
struct Memory {
    jobs: HashMap<JobId, JobTracker>,
}

impl Coordinator for MemoryCoordinator {
    /// spawns a new tokio child. a job tracker is also started which will monitor the child
    /// process and consume its output streams.
    async fn spawn(&self, ctx: Ctx, spec: JobSpec) -> Result<Job> {
        let job_id = JobId::default();
        info!("spawning job {job_id}");
        let user = ctx.user.clone();
        let child = spawn::Opts::new(spec).spawn().map_err(Error::Spawn)?;
        let pid = Pid::new(child.pid());
        let job = Job::started(&job_id, &user, &pid);
        let tracker = JobTracker::track(&job, child);
        if self
            .inner
            .lock()
            .await
            .jobs
            .insert(job_id.clone(), tracker.clone())
            .is_some()
        {
            panic!("job with id {job_id} already being tracked");
        }
        Ok(job)
    }

    /// fetches the tracker for the specified job. the tracker supplies the log receiver for the
    /// job which is then returned.
    async fn logs(&self, ctx: Ctx, job_id: JobId) -> Result<Receiver<FrameEvent>> {
        let (tracker, _) = self.get_tracker(&ctx, &job_id).await?;
        let rx = tracker.stream_logs().await.map_err(logs_unavail)?;
        Ok(rx)
    }

    /// fetches a snapshot of the specified job
    async fn ps(&self, ctx: Ctx, job_id: JobId) -> Result<Job> {
        let (_, job) = self.get_tracker(&ctx, &job_id).await?;
        Ok(job)
    }

    /// Attempts to kill the specific job.
    async fn kill(&self, ctx: Ctx, job_id: JobId) -> Result<Job> {
        let (tracker, _) = self.get_tracker(&ctx, &job_id).await?;
        match tracker.kill(&ctx).await {
            Ok(job) => Ok(job),
            Err(KillError::ReqSendFailed) => Err(job_not_running(&job_id)),
            Err(err) => Err(Error::Kill(err)),
        }
    }
}

impl MemoryCoordinator {
    /// gets the job tracker and current job snapshot for the specified job id. if the tracker was
    /// found but it does not match the owner in the ctx, we will error with not_found to avoid bad
    /// actors discovering job ids.
    async fn get_tracker(&self, ctx: &Ctx, job_id: &JobId) -> Result<(JobTracker, Job)> {
        let tracker = self.inner.lock().await.jobs.get(job_id).cloned();
        let Some(tracker) = tracker else {
            return Err(not_found(job_id));
        };
        let job = tracker.job().await;
        if job.user.id != ctx.user.id {
            // intentionally ambiguous to avoid bad actors discovering job ids
            return Err(not_found(job_id));
        }
        Ok((tracker, job))
    }
}

#[derive(thiserror::Error, Debug)]
pub enum TrackerError {
    #[error("failed to wait for child process: {0}")]
    WaitChild(#[source] io::Error),

    #[error("read {kind}: {err}")]
    ReadOutput {
        kind: StreamKind,
        #[source]
        err: io::Error,
    },

    #[error("broadcaster disappeared: {err}")]
    BroadcasterDisappeared { err: SendError<FrameEvent> },
}

/// The job tracker handles keeping track of all of the runtime state of a job and its associated
/// child process. Cloning is cheap.
///
/// NB: currently all of the tracker state is stored in inner. it might be worth considering
/// putting the Job state in a separate mutex if contention for the job tracker is high, which it
/// should not be currently.
#[derive(Clone)]
struct JobTracker {
    inner: Arc<Mutex<JobTrackerInner>>,
}

struct JobTrackerInner {
    job: Job, // the job snapshot. updated by the tracker periodically
    res: Option<Result<(), Arc<TrackerError>>>, // currently updated but not used
    kill_tx: Sender<KillRequest>, // requests to kill are send on this chan
    bcast: FrameBroadcaster, // broadcasts log streams on demand
}

impl JobTracker {
    /// creates a new job tracker for a newly spawned child process and spawns a future to manage
    /// that process. a framebroadcaster is also created which will receive output frames from the
    /// child stdout and stderr streams. the broadcaster can then relay them to clients on demand.
    fn track(job: &Job, details: Box<dyn spawn::Child>) -> Self {
        let (frame_tx, frame_rx) = mpsc::channel(2); // for child output
        let (kill_tx, kill_rx) = mpsc::channel(1); // for kill requests
        let bcast = FrameBroadcaster::start(frame_rx);
        let tracker = JobTracker {
            inner: Arc::new(Mutex::new(JobTrackerInner {
                job: job.clone(),
                res: None,
                kill_tx,
                bcast,
            })),
        };
        tokio::spawn({
            let tracker = tracker.clone();
            tracker.wait(details, frame_tx, kill_rx)
        });
        tracker
    }

    /// the main control routine for a spawned job. this JobTracker method is a glorified select
    /// loop in which the following actions will be taken:
    ///
    /// * frames read from stdout will be sent to the broadcaster via frame_tx.
    /// * frames read from stderr will be send tot he broadcaster via frame_tx.
    /// * i/o errors encountered while reading stdout/stderr frames will cause the job to fail.
    /// * incoming requests to kill the job will result in a child.kill() attempt.
    /// * when the child process exits, the appropriate bookkeeping will be done.
    ///
    /// once the child process is no longer running, this method will exit. the broadcaster will
    /// still be attached to the job tracker, allowing future log API requests to fetch the output
    /// of the child process.
    #[instrument(skip_all)]
    async fn wait(
        self,
        mut child: Box<dyn spawn::Child>,
        frame_tx: Sender<FrameEvent>,
        mut kill_rx: Receiver<KillRequest>,
    ) {
        debug!("start.");
        let mut stdout = Self::output_rx(child.stdout(), StreamKind::Stdout);
        let mut stderr = Self::output_rx(child.stderr(), StreamKind::Stderr);
        let mut stdout_ok = true;
        let mut stderr_ok = true;
        let mut kill_ok = true;
        let mut child_ok = true;
        let mut loop_err: Option<TrackerError> = None;
        let mut kill_reqs = vec![];
        // breaks out of the loop, setting the loop_err along the way. this is intended to make the
        // main tokio select loop a bit more readable and ensuring that the proper bookkeeping is
        // done.
        macro_rules! break_err {
            ($err:expr) => {{
                let err = $err;
                error!("breaking out of loop: {}", err);
                loop_err.replace(err);
                break;
            }};
        }
        // unpacks a frame from either stdout or stderr. if the frame was None then the ok var is
        // set to false to disable it. if the frame was an error, then we break out of the loop.
        macro_rules! unpack_frame {
            ($ev:expr, $ok:ident) => {{
                let Some(ev) = $ev else {
                    $ok = false;
                    continue;
                };
                match ev {
                    Ok(frame) => FrameEvent::Frame(Arc::new(frame)),
                    Err(err) => break_err!(err),
                }
            }};
        }
        // the main select loop. listen for new stdout/stderr frames and send them to the
        // broadcaster. if a kill request arrives, try to kill the child process. otherwise, wait
        // for the process to die and then terminate.
        loop {
            debug!("loop start.");
            if !child_ok && !stdout_ok && !stderr_ok {
                debug!("child process exited and output streams collected.");
                break;
            }

            tokio::select! {
                ev = stdout.recv(), if stdout_ok => {
                    debug!("stdout frame: {ev:?}");
                    let frame = unpack_frame!(ev, stdout_ok);
                    if let Err(err) = frame_tx.send(frame).await {
                        break_err!(TrackerError::BroadcasterDisappeared { err });
                    }
                }
                ev = stderr.recv(), if stderr_ok => {
                    debug!("stderr frame: {ev:?}");
                    let frame = unpack_frame!(ev, stderr_ok);
                    if let Err(err) = frame_tx.send(frame).await {
                        break_err!(TrackerError::BroadcasterDisappeared { err });
                    }
                }
                kill_req = kill_rx.recv(), if kill_ok => {
                    debug!("kill req: {kill_req:?}");
                    let Some(req) = kill_req else {
                        kill_ok = false;
                        continue;
                    };
                    if let Err(err) = child.kill().await {
                        let _ = req.tx.send(Err(err));
                        continue;
                    }
                    // we successfully killed the child process. set the killed-by metadata and
                    // re-enter the select loop to await the child exiting.
                    {
                        let mut inner = self.inner.lock().await;
                        inner.job.killed = Some(JobKilled {
                            killed_at: Utc::now(),
                            user: req.user.clone(),
                        });
                    }
                    // push retain the kill req so that we can send on it when the process dies.
                    kill_reqs.push(req);
                }
                status = child.wait(), if child_ok => {
                    debug!("child exit: {status:?}");
                    child_ok = false;
                    match status {
                        Ok(status) => {
                            // update the job metadata
                            let job = {
                                let mut inner = self.inner.lock().await;
                                inner.job.ended_at = Some(Utc::now());
                                inner.job.state = JobState::Exited {
                                    status: status.into(),
                                };
                                inner.job.clone()
                            };
                            // send the job state to the kill requests
                            for req in kill_reqs.drain(..) {
                                let _ = req.tx.send(Ok(job.clone()));
                            }
                        },
                        Err(err) => break_err!(TrackerError::WaitChild(err)),
                    }
                }
            }
        }
        debug!("end select");
        if let Some(err) = loop_err {
            // send an error event to the frame broadcaster.
            let err = Arc::new(err);
            if let Err(err) = frame_tx.send(FrameEvent::Error(err.clone())).await {
                error!("Failed to send to broadcaster: {err}");
            }
            self.inner.lock().await.res.replace(Err(err));
        } else {
            // a normal exit results in an EOF sent to the broadcaster.
            if let Err(err) = frame_tx.send(FrameEvent::EOF).await {
                error!("Failed to send to broadcaster: {err}");
            }
            self.inner.lock().await.res.replace(Ok(()));
        }
        debug!("complete.");
    }

    /// Converts the supplied stream into a reciver of frame results
    #[instrument(skip(s))]
    fn output_rx<S>(s: S, kind: StreamKind) -> Receiver<Result<Frame, TrackerError>>
    where
        S: AsyncRead + Unpin + Send + 'static,
    {
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(Self::consume_output(s, kind, tx));
        rx
    }

    #[instrument(skip(s, tx))]
    async fn consume_output<S>(mut s: S, kind: StreamKind, tx: Sender<Result<Frame, TrackerError>>)
    where
        S: AsyncRead + Unpin + Send + 'static,
    {
        let consume = |tx: Sender<Result<Frame, TrackerError>>| async move {
            loop {
                let mut buf = vec![0; 1024];
                let n = s
                    .read(&mut buf[..])
                    .await
                    .map_err(|err| TrackerError::ReadOutput { kind, err })?;
                debug!("Read {n} bytes");
                if n == 0 {
                    break;
                }
                let frame = Frame::new(kind, &buf[0..n]);
                if tx.send(Ok(frame)).await.is_err() {
                    break;
                }
                debug!("Sent frame to receiver");
            }
            Result::Ok(())
        };
        if let Err(err) = consume(tx.clone()).await {
            let _ = tx.send(Err(err)).await;
        }
    }

    /// send a kill request to the job tracker
    async fn kill(&self, ctx: &Ctx) -> Result<Job, KillError> {
        let (req, rx) = KillRequest::new(ctx);
        let tx = self.inner.lock().await.kill_tx.clone();
        tx.send(req).await.map_err(|_| KillError::ReqSendFailed)?;
        tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .map_err(KillError::ReqTimeout)?
            .map_err(KillError::TxDropped)?
            .map_err(KillError::Failed)
    }

    async fn job(&self) -> Job {
        self.inner.lock().await.job.clone()
    }

    async fn stream_logs(&self) -> Result<Receiver<FrameEvent>, FrameError> {
        let bcast = self.inner.lock().await.bcast.clone();
        bcast.stream().await
    }
}

#[derive(thiserror::Error, Debug)]
pub enum KillError {
    #[error("sending of kill failed. child might have quit")]
    ReqSendFailed,

    #[error("timed out waiting for child kill")]
    ReqTimeout(Elapsed),

    #[error("tracker quit while trying to kill child: {0}")]
    TxDropped(oneshot::error::RecvError),

    #[error("attempt to kill child failed: {0}")]
    Failed(io::Error),
}

/// a request to kill a process. the result will be send on the specified channel
struct KillRequest {
    user: User,
    tx: oneshot::Sender<io::Result<Job>>,
}

impl std::fmt::Debug for KillRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KillRequest")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

impl KillRequest {
    fn new(ctx: &Ctx) -> (Self, oneshot::Receiver<io::Result<Job>>) {
        let (tx, rx) = oneshot::channel();
        let req = Self {
            user: ctx.user.clone(),
            tx,
        };
        (req, rx)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum FrameError {
    #[error("could not track logs because the broadcaster quit")]
    BroadcasterUnavailableForLogs(SendError<FrameRequest>),

    #[error("timed out waiting for a log stream from broadcaster")]
    TimeoutWaitingForLogRx,

    #[error("failed to receive log stream from broadcaster: {0}")]
    GetLogStream(#[source] oneshot::error::RecvError),
}

/// The broadcaster receives frames from the child's stdout and stderr streams. Each frame is
/// buffered locally. Requests to relay those frames to clients are converted into FrameRelay's
/// which receive the existing buffered frames, and will receive any new frames as they arrive.
///
/// When there are no more frames from the child process, existing frame relays will be dropped,
/// allowing them to signal to their clients that there is no more log output to stream. New
/// requests for frames will be started and immediately dropped, causing them to send all of the
/// buffered frames without waiting for new ones.
#[derive(Clone)]
struct FrameBroadcaster {
    tx: Sender<FrameRequest>,
}

impl FrameBroadcaster {
    /// starts a new broadcaster with the supplied FrameEvent receiver. The broadcaster's run
    /// method will be run in a separate tokio task and will accumulate output frames and respond
    /// to requests for those frames. The returned broadcaster composes a FrameRequest sender to
    /// communicate with the tokio task.
    fn start(source: Receiver<FrameEvent>) -> Self {
        let (req_tx, req_rx) = mpsc::channel(1);
        let fb = Self { tx: req_tx };
        tokio::spawn({
            let fb = fb.clone();
            fb.bcast(source, req_rx)
        });
        fb
    }

    /// the main control loop for the frame broadcaster. like the JobTracker::wait method, this is
    /// also a select loop in which the following actions will be taken:
    ///
    /// * incoming FrameEvents from the receiver will be buffered locally.
    /// * incoming FrameRequests will result in FrameRelays being created for each request
    /// * as incoming frames are received, they will be relayed to the FrameRelays
    #[instrument(skip_all)]
    async fn bcast(self, mut frame_rx: Receiver<FrameEvent>, mut req_rx: Receiver<FrameRequest>) {
        debug!("start.");
        let mut frames: Vec<FrameEvent> = vec![];
        let mut relays: Vec<FrameRelay> = vec![];
        let mut frame_rx_ok = true;
        let mut req_rx_ok = true;
        loop {
            debug!("loop start.");
            if !frame_rx_ok && !req_rx_ok {
                // no more work to be done
                break;
            }
            tokio::select! {
                ev = frame_rx.recv(), if frame_rx_ok => {
                    debug!("frame rx: {ev:?}");
                    // a new frame event is received from the job tracker.
                    let Some(ev) = ev else {
                        // no more frames will be received from the tracker.
                        debug!("clearing relays");
                        frame_rx_ok = false;
                        relays.clear();
                        continue;
                    };
                    relays.retain(|r| {
                        let ok = r.accept(ev.clone());
                        if !ok { debug!("relay did not accept"); }
                        ok
                    });
                    frames.push(ev);
                }
                req = req_rx.recv(), if req_rx_ok => {
                    debug!("req rx: {req:?}");
                    // a new request for frames is received.
                    let Some(req) = req else {
                        debug!("no more relay rx");
                        req_rx_ok = false;
                        continue;
                    };
                    debug!("Starting frame relay...");
                    // start a new framerelay with a snapshot of the current frames.
                    if let Some(relay) = FrameRelay::start(frames.clone(), req) {
                        if frame_rx_ok {
                            // we only need to track the relay if there might be more frames.
                            relays.push(relay);
                        }
                    }
                }
            }
        }
        debug!("end.");
    }

    /// creates a new consumer for the frames. received frames will start at the beginning.
    #[instrument(skip(self))]
    async fn stream(&self) -> Result<Receiver<FrameEvent>, FrameError> {
        debug!("Setting up log stream..");
        let (req, rx) = FrameRequest::new();
        self.tx
            .send(req)
            .await
            .map_err(FrameError::BroadcasterUnavailableForLogs)?;
        debug!("Sent frame request to broadcaster successfully");
        let rx = match tokio::time::timeout(Duration::from_secs(1), rx).await {
            Ok(Ok(rx)) => rx,
            Ok(Err(err)) => return Err(FrameError::GetLogStream(err)),
            Err(_) => return Err(FrameError::TimeoutWaitingForLogRx),
        };
        debug!("Returning log stream receiver to caller");
        Ok(rx)
    }
}

/// the frame relay is used by the broadcaster whenever a new request comes in to read frames. the
/// relay will playback all existing frames in addition to any new ones that arrive. in addition,
/// the relay will buffer messages locally to avoid backups with slow receivers. the frames are
/// reference counted which should reduce overhead.
#[derive(Clone)]
struct FrameRelay {
    bcast_tx: Sender<FrameEvent>, // the broadcaster will send on this
}

impl FrameRelay {
    /// When a relay is started, it starts with a snapshot of buffered frames, and the returned
    /// relay has a sender which can receive new frames from the broadcaster.
    ///
    /// the frame request has a oneshot that allows the receiver to be sent to the client. if this
    /// send to the client fails, then there is no need to start the relay, and in this case, None
    /// is returned.
    #[instrument(skip_all)]
    fn start(frames: Vec<FrameEvent>, req: FrameRequest) -> Option<Self> {
        debug!("Starting frame relay with {} frames", frames.len());

        // this channel is between the broadcaster and the frame relay. the broadcaster will
        // perform non-blocking sends to each relay, dropping a relay if the send fails. we want
        // this channel to have enough of a buffer to ensure that if the relay is currently dealing
        // with the communication to the client that it is not dropped unfairly.
        //
        // the alternative here would be to use a blocking send with a timeout when the broadcaster
        // attempts to hand a frame to the relay.
        let (bcast_tx, bcast_rx) = mpsc::channel(1024);

        // we also buffer the channel in between the relay and the client. For both the broadcaster
        // and relay channels, the FrameEvents are fairly cheap since they are reference counted,
        // so it's OK to have a somewhat large buffer (1024 items).
        let (relay_tx, relay_rx) = mpsc::channel(1024);

        if req.response.send(relay_rx).is_err() {
            warn!("relay: client did not accept the receiver");
            return None;
        }
        let relay = Self { bcast_tx };
        tokio::spawn({
            let relay = relay.clone();
            relay.relay(frames, relay_tx, bcast_rx)
        });
        Some(relay)
    }

    #[instrument(skip_all)]
    async fn relay(
        self,
        frames: Vec<FrameEvent>,            // the initial set of frames
        frame_tx: Sender<FrameEvent>,       // to the client
        mut frame_rx: Receiver<FrameEvent>, // from the broadcaster
    ) {
        debug!("start.");
        let mut frames = VecDeque::from(frames);
        let mut to_send: Option<FrameEvent> = None;
        let mut drain = false;
        let send_frame = |ev: FrameEvent| async {
            debug!("send {ev:?}");
            frame_tx
                .send(ev)
                .await
                .inspect_err(|err| warn!("send failed: {err}"))
        };
        // each iteration we either try to send the next frame to the client or accept a new
        // frame from the broadcaster. frames received from the broadcaster are buffered
        // locally.
        loop {
            if to_send.is_none() {
                to_send = frames.pop_front();
            }
            tokio::select! {
                frame = frame_rx.recv() => {
                    if let Some(frame) = frame {
                        debug!("push {frame:?}");
                        frames.push_back(frame);
                    } else {
                        debug!("no more frames from broadcaster");
                        drain = true;
                        break;
                    }
                }
                res = async { send_frame(to_send.clone().unwrap()).await }, if to_send.is_some() => {
                    if res.is_err() {
                        // the client dropped
                        debug!("client dropped");
                        break;
                    }
                    to_send = None;
                }
            }
        } // end loop
        if drain {
            debug!("drain...");
            for frame in to_send.into_iter().chain(frames) {
                let _ = send_frame(frame).await;
            }
        }
        debug!("end.");
    }

    /// offer a frame to this frame relay. called by the broadcaster.
    fn accept(&self, frame: FrameEvent) -> bool {
        let res = self.bcast_tx.try_send(frame);
        if let Err(err) = &res {
            warn!("accept: frame relay could not send: {err}");
        }
        res.is_ok()
    }
}

/// a request to start receiving frames. the receiver will be sent on the supplied oneshot channel.
pub struct FrameRequest {
    response: oneshot::Sender<Receiver<FrameEvent>>,
}

impl std::fmt::Debug for FrameRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameRequest").finish_non_exhaustive()
    }
}

impl FrameRequest {
    fn new() -> (Self, oneshot::Receiver<Receiver<FrameEvent>>) {
        let (tx, rx) = oneshot::channel();
        let res = Self { response: tx };
        (res, rx)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
}

impl fmt::Display for StreamKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamKind::Stdout => write!(f, "stdout"),
            StreamKind::Stderr => write!(f, "stderr"),
        }
    }
}

pub struct Frame {
    pub kind: StreamKind,
    pub bs: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("kind", &self.kind)
            .field("len", &self.bs.len())
            .finish_non_exhaustive()
    }
}

/// FrameEvents are transmitted to clients of the logs API. Individual frames are immutable, and as
/// such are reference counted to reduce memory usage.
#[derive(Clone)]
pub enum FrameEvent {
    /// an output frame became available
    Frame(Arc<Frame>),
    /// an error occurred. this is a terminal variant.
    Error(Arc<TrackerError>),
    /// no more frames. this is a terminal variant.
    EOF,
}

impl std::fmt::Debug for FrameEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameEvent::Frame(frame) => write!(f, "{frame:?}"),
            FrameEvent::Error(err) => write!(f, "ERR: {err}"),
            FrameEvent::EOF => write!(f, "EOF"),
        }
    }
}

impl Frame {
    fn new(kind: StreamKind, bs: &[u8]) -> Self {
        let bs = bs.to_vec();
        Self { kind, bs }
    }
}

#[cfg(test)]
mod tests {
    use core::panic;

    use super::*;
    use crate::proc::types::User;
    use chrono::Utc;
    use tokio::time::{sleep, timeout};
    use tracing_test::traced_test;

    macro_rules! demux_logs {
        ($rx:ident) => {{
            let demux = timeout(Duration::from_secs(5), FrameDemux::consume($rx))
                .await
                .expect("timed out while demuxing");
            demux
        }};
    }

    #[traced_test]
    #[tokio::test]
    async fn simple() {
        let now = Utc::now();
        let coord = MemoryCoordinator::default();
        let ctx = Ctx::from("test-user");
        let cmd = JobSpec::new("echo").arg("foobar");
        let job = coord.spawn(ctx.clone(), cmd).await.unwrap();
        assert!(!job.id.is_empty());
        assert_eq!(job.user, User::from("test-user"));
        assert!(job.started_at >= now);
        assert!(job.killed.is_none());
        assert!(job.ended_at.is_none());
        assert!(matches!(job.state, JobState::Running { .. }));

        // verify the log output
        let log_rx = coord.logs(ctx.clone(), job.id.clone()).await.unwrap();
        let demux = demux_logs!(log_rx);
        assert_eq!(demux, ("foobar\n", ""));
    }

    #[traced_test]
    #[tokio::test]
    async fn coord_general() {
        let now = Utc::now();
        let coord = MemoryCoordinator::default();
        let ctx = Ctx::from("test-user");
        let cmd = JobSpec::new("echo").arg("foobar");
        let job = coord.spawn(ctx.clone(), cmd).await.unwrap();
        assert!(!job.id.is_empty());
        assert_eq!(job.user, User::from("test-user"));
        assert!(job.started_at >= now);
        assert!(job.killed.is_none());
        assert!(job.ended_at.is_none());
        assert!(matches!(job.state, JobState::Running { .. }));

        // verify the log output
        let log_rx = coord.logs(ctx.clone(), job.id.clone()).await.unwrap();
        let demux = demux_logs!(log_rx);
        assert_eq!(demux, ("foobar\n", ""));

        // a different user should not be able to get the logs
        assert!(matches!(
            coord.logs("bad-actor".into(), job.id.clone()).await,
            Err(Error::JobNotFound { .. })
        ));

        // wait for the job to end and verify its state.
        let job = timeout(Duration::from_secs(1), wait_on_job(&ctx, &job, &coord))
            .await
            .expect("while waiting for job");

        assert!(!job.id.is_empty());
        assert_eq!(job.user, User::from("test-user"));
        assert!(job.started_at >= now);
        assert!(job.ended_at.is_some_and(|t| t >= job.started_at),);
        assert!(job.killed.is_none());
        assert_eq!(job.state, JobState::exited(0), "{job:#?}");

        // a different user should not be able to ps this job
        assert!(matches!(
            coord.ps("bad-actor".into(), job.id.clone()).await,
            Err(Error::JobNotFound { .. })
        ));

        // if we try to kill the job, it should fail because it's not running.
        let killed = coord.kill(ctx.clone(), job.id.clone()).await;
        assert!(
            matches!(killed, Err(Error::JobNotRunning { .. }),),
            "{killed:#?}"
        );

        // a different user should not be able to kill this job
        assert!(matches!(
            coord.kill("bad-actor".into(), job.id.clone()).await,
            Err(Error::JobNotFound { .. })
        ));
    }

    // verify that the logs can be fetched more than once
    #[traced_test]
    #[tokio::test]
    async fn get_logs_multiple_times() {
        let coord = MemoryCoordinator::default();
        let ctx = Ctx::from("test-user");
        let cmd = JobSpec::new("echo").arg("foobar");
        let job = coord.spawn(ctx.clone(), cmd).await.unwrap();
        for _ in 0..10 {
            let log_rx = coord.logs(ctx.clone(), job.id.clone()).await.unwrap();
            let demux = demux_logs!(log_rx);
            assert_eq!(demux, ("foobar\n", ""));
        }
    }

    // even after a job terminates, we can still get the logs
    #[tokio::test]
    #[traced_test]
    async fn get_logs_after_process_finished() {
        let coord = MemoryCoordinator::default();
        let ctx = Ctx::from("test-user");
        let cmd = JobSpec::new("echo").arg("foobar");
        let job = coord.spawn(ctx.clone(), cmd).await.unwrap();
        assert!(job.state.is_running());
        // wait for the job to finish
        let job = timeout(Duration::from_secs(1), wait_on_job(&ctx, &job, &coord))
            .await
            .expect("timed out waiting for job");
        // verify we can still get the logs
        assert!(!job.state.is_running());
        let log_rx = coord.logs(ctx, job.id.clone()).await.unwrap();
        let demux = demux_logs!(log_rx);
        assert_eq!(demux, ("foobar\n", ""));
    }

    // a job can be killed when it's in the running state
    //
    // this currently targets macos until we get a cross platform test running
    #[tokio::test]
    #[traced_test]
    #[cfg(target_os = "macos")]
    async fn test_kill() {
        let now = Utc::now();
        let coord = MemoryCoordinator::default();
        let ctx = Ctx::from("test-user");
        let cmd = JobSpec::new("sh").args(["-c", "echo foo; sleep 5"]);
        let job = coord.spawn(ctx.clone(), cmd).await.unwrap();

        // sleep a little bit to allow the child to log to stdout. we'll test for this output after
        // we kill the child. behind feature flag because it can be expensive.
        #[cfg(feature = "integration_test")]
        tokio::time::sleep(Duration::from_secs(500)).await;

        // ps the job to verify it is running
        let job = coord.ps(ctx.clone(), job.id.clone()).await.unwrap();
        assert!(job.state.is_running(), "{job:#?}");

        // kill the job
        let job = coord.kill(ctx.clone(), job.id.clone()).await.unwrap();
        assert!(job.state.is_exited(), "{job:#?}");
        let killed = job.killed.clone().unwrap();
        assert_eq!(killed.user, ctx.user);
        assert!(killed.killed_at > now);
        assert!(
            matches!(job.state, JobState::Exited { .. }),
            "{:#?}",
            job.state
        );

        // killing the job again should fail. the exact failure depends on the race between the
        // original kill and whether or not the job tracker has noticed that the job has exited.
        // so we will just verify that the kill itself is not OK.
        let kill = coord.kill(ctx.clone(), job.id.clone()).await;
        assert!(kill.is_err(), "{kill:#?}");

        // when we kill the child process, the os can take a while to close its output streams.
        // gate this part of the test behind a feature flag to allow for faster tests
        #[cfg(feature = "integration_test")]
        {
            let log_rx = coord.logs(ctx, job.id.clone()).await.unwrap();
            let demux = timeout(Duration::from_secs(10), FrameDemux::consume(log_rx))
                .await
                .expect("timed out while demuxing");
            assert_eq!(demux, ("foo\n", ""));
        }
    }

    // blocks until the job moves out of the running state.
    async fn wait_on_job(ctx: &Ctx, job: &Job, coord: &MemoryCoordinator) -> Job {
        loop {
            let job = coord.ps(ctx.clone(), job.id.clone()).await.unwrap();
            if job.state.is_running() {
                sleep(Duration::from_millis(10)).await;
                continue;
            }
            return job;
        }
    }

    impl From<&'static str> for Ctx {
        fn from(value: &'static str) -> Self {
            Ctx {
                user: User {
                    id: value.to_string(),
                },
            }
        }
    }

    /// takes a receiver of frames and demuxes them into stdout and stderr byte vecs.
    #[derive(Default, Debug)]
    struct FrameDemux {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    }

    impl PartialEq<(&str, &str)> for FrameDemux {
        fn eq(&self, other: &(&str, &str)) -> bool {
            let stdout = other.0;
            let stderr = other.1;
            if self.stdout != stdout.as_bytes() {
                return false;
            }
            if self.stderr != stderr.as_bytes() {
                return false;
            }
            true
        }
    }

    impl FrameDemux {
        #[instrument(name = "demux", skip_all)]
        async fn consume(mut rx: Receiver<FrameEvent>) -> Self {
            let mut demux = Self::default();
            loop {
                debug!("loop start.");
                let Some(event) = rx.recv().await else {
                    debug!("could not receive");
                    panic!("frame relay disappeared unexpectedly");
                };
                debug!("got an event");
                debug!("Demux event: {event:?}");
                match event {
                    FrameEvent::Frame(frame) => {
                        let bs = frame.bs.clone();
                        if frame.kind == StreamKind::Stdout {
                            demux.stdout.extend(bs);
                        } else {
                            demux.stderr.extend(bs);
                        }
                    }
                    FrameEvent::Error(err) => {
                        panic!("frame error: {err}");
                    }
                    FrameEvent::EOF => {
                        break;
                    }
                };
            }
            demux
        }
    }
}
