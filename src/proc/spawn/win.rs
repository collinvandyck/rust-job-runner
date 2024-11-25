//! Windows specific code to handle spawning jobs alongside job objects

use futures::{task::AtomicWaker, Future};
use std::{
    ffi::OsString,
    io, mem,
    ops::{Deref, DerefMut},
    os::windows::{ffi::OsStrExt, process::ExitStatusExt},
    pin::Pin,
    process::ExitStatus,
    sync::{mpsc, Arc, Mutex},
    task::{Context, Poll, Waker},
    thread,
};
use tokio::io::{AsyncRead, ReadBuf};
use tracing::{debug, instrument, warn};
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{
            CloseHandle, SetHandleInformation, ERROR_BROKEN_PIPE, HANDLE, HANDLE_FLAG_INHERIT,
            WAIT_OBJECT_0,
        },
        Storage::FileSystem::ReadFile,
        System::{
            Pipes::CreatePipe,
            Threading::{
                CreateProcessW, GetExitCodeProcess, TerminateProcess, WaitForSingleObject,
                INFINITE, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
                STARTUPINFOW,
            },
        },
    },
};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to create process: {0}")]
    CreateProcessW(#[source] windows_result::Error),

    #[error("could not create read pipe: {0}")]
    CreateReadPipe(#[source] windows_result::Error),

    #[error("failed to read from {kind:?}: {err}")]
    ReadFile {
        kind: PipeKind,
        #[source]
        err: windows_result::Error,
    },
}

type Result<T, E = Error> = std::result::Result<T, E>;

/// optionally "owns" a HANDLE. the HANDLE closed when the Handle is dropped, if the HANDLE is
/// owned. also contains a diagnotic "kind" to make tracing events more meaningful.
#[derive(Debug)]
struct Handle {
    owned: bool,
    kind: HandleKind,
    inner: HANDLE,
}

/// cloning a handle converts it into a non-owned version to prevent double-close.
impl Clone for Handle {
    fn clone(&self) -> Self {
        Self {
            owned: false,
            kind: self.kind,
            inner: self.inner,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum HandleKind {
    Process,
    Thread,
    // used in debug impl but clippy complains
    #[allow(unused)]
    PipeRx(PipeKind),
    // used in debug impl but clippy complains
    #[allow(unused)]
    PipeTx(PipeKind),
}

impl Handle {
    fn owned(inner: HANDLE, kind: HandleKind) -> Self {
        Self {
            owned: true,
            kind,
            inner,
        }
    }

    #[instrument(name = "Handle::close", skip_all)]
    fn close(&mut self) {
        if !self.owned {
            debug!("not closing non-owned handle {self:?}");
            return;
        }
        // debug log which particular handle this is, using the kind field.
        debug!("closing {self:?}");
        if !self.inner.is_invalid() {
            unsafe { CloseHandle(self.inner).expect("close handle failed") }
        }
    }
}

// Here we implement Send unsafely for Handle. Handle composes a HANDLE which should be safe to
// send between threads, but the way it is represented under the hood uses a void pointer which is
// unsafe to send in Rust. From what I gather, implementing Send here is safe for this particular
// circumstance. By implementing it for Handle, we do not have to implement it for the other
// various types which compose a Handle.
unsafe impl Send for Handle {}

impl Deref for Handle {
    type Target = HANDLE;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Handle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.close();
    }
}

/// The wrapper around the data returned by a successful CreateProcessW call.
///
/// TODO: implement kill_on_drop functionality.
pub struct Child {
    proc_info: ProcessInfo,
    stdout: Option<Handle>,
    stderr: Option<Handle>,
}

impl Child {
    #[instrument(skip_all)]
    pub fn spawn(opts: super::Opts) -> Result<Self, Error> {
        let (stdout_tx, stdout_rx) = PipeKind::Stdout.create_handles()?;
        let (stderr_tx, stderr_rx) = PipeKind::Stderr.create_handles()?;
        let mut cmd_line = WinArgs::new(opts.spec.exe_and_args());
        let lpprocessattributes = None;
        let lpthreadattributes = None;
        let binherithandles = true;
        let dwcreationflags = PROCESS_CREATION_FLAGS::default();
        let lpenvironment = None;
        let lpcurrentdirectory = None;
        let cb = mem::size_of::<STARTUPINFOW>()
            .try_into()
            .expect("STARTUPINFOW size did not fit into u32");
        let si = STARTUPINFOW {
            cb,
            hStdOutput: *stdout_tx,
            hStdError: *stderr_tx,
            dwFlags: STARTF_USESTDHANDLES,
            ..Default::default()
        };
        let mut proc_info = PROCESS_INFORMATION::default();
        unsafe {
            CreateProcessW(
                PWSTR::null(),
                cmd_line.as_pwstr(),
                lpprocessattributes,
                lpthreadattributes,
                binherithandles,
                dwcreationflags,
                lpenvironment,
                lpcurrentdirectory,
                &si,
                &mut proc_info,
            )
            .map_err(Error::CreateProcessW)?;
        }
        // if the process is created, these assertions should always be true.
        // we panic if they prove to be false.
        assert!(!proc_info.hProcess.is_invalid());
        assert!(!proc_info.hThread.is_invalid());
        assert!(proc_info.dwProcessId > 0);
        assert!(proc_info.dwThreadId > 0);
        // close the write side of the pipes so that reads can happen. happens after CreateProcessW.
        debug!("Dropping tx side of stdout and stderr pipes.");
        drop(stdout_tx);
        drop(stderr_tx);
        Ok(Child {
            proc_info: proc_info.into(),
            stdout: Some(stdout_rx),
            stderr: Some(stderr_rx),
        })
    }
}

impl super::Child for Child {
    fn pid(&self) -> u32 {
        self.proc_info.pid
    }

    fn stdout(&mut self) -> Box<dyn super::OutputStream> {
        let stdout = self.stdout.take().expect("stdout handle already taken");
        let stdout = WinHandleOutputStream::start(stdout);
        Box::new(stdout)
    }

    fn stderr(&mut self) -> Box<dyn super::OutputStream> {
        let stderr = self.stderr.take().expect("stderr handle already taken");
        let stderr = WinHandleOutputStream::start(stderr);
        Box::new(stderr)
    }

    fn wait(&mut self) -> super::WaitReturn<'_> {
        let handle = self.proc_info.process.clone();
        let wait = WaitChild::new(handle);
        Box::pin(wait)
    }

    fn kill(&mut self) -> super::KillReturn<'_> {
        let handle = self.proc_info.process.clone();
        let kill = KillChild::new(handle);
        Box::pin(kill)
    }
}

/// KillChild is a future which kills the child process.
///
/// NB: this future is not cancellation safe, as it spawns a background thread to kill the process
/// when polled to perform the call to TerminateProcess.
#[derive(Clone)]
struct KillChild {
    handle: Handle,
    waker: Arc<AtomicWaker>,
    rx: Option<Arc<Mutex<mpsc::Receiver<io::Result<()>>>>>,
}

impl KillChild {
    fn new(handle: Handle) -> Self {
        Self {
            handle,
            rx: None,
            waker: Arc::new(AtomicWaker::new()),
        }
    }
}

impl Future for KillChild {
    type Output = io::Result<()>;

    #[instrument(name = "KillChild::poll", skip_all)]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.waker.register(cx.waker());
        match &self.rx {
            None => {
                debug!("creating kill thread.");
                let (tx, rx) = mpsc::sync_channel(0);
                thread::spawn({
                    let kc = self.clone();
                    let waker = self.waker.clone();
                    move || {
                        let handle = kc.handle;
                        let res = unsafe { TerminateProcess(*handle, 1) };
                        let res = res.map_err(io::Error::other);
                        if let Err(err) = tx.send(res) {
                            debug!("rx dropped sending terminate result: {err}");
                        }
                        waker.wake();
                    }
                });
                self.rx.replace(Arc::new(Mutex::new(rx)));
                Poll::Pending
            }
            Some(rx) => {
                let rcv = rx.lock().unwrap().try_recv();
                match rcv {
                    Ok(Ok(())) => Poll::Ready(Ok(())),
                    Ok(Err(io_err)) => Poll::Ready(Err(io_err)),
                    Err(mpsc::TryRecvError::Empty) => {
                        debug!("empty read from receiver");
                        Poll::Pending
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        debug!("receiver disconnected");
                        Poll::Ready(Ok(()))
                    }
                }
            }
        }
    }
}

/// WaitChild is a future which performs a similar operation that the stdlib and tokio Child::wait
/// APIs provide, but it instead uses the windows WaitForSingleObject API with the composed HANDLE
/// to do this more natively.
#[derive(Clone)]
struct WaitChild(Handle);

impl WaitChild {
    fn new(handle: Handle) -> Self {
        Self(handle)
    }
}

impl WaitChild {
    #[instrument(skip_all)]
    fn wait_wake(self, waker: Waker) {
        debug!("waiting...");
        unsafe { WaitForSingleObject(*self.0, INFINITE) };
        debug!("process state changed.");
        waker.wake();
    }
}

impl Future for WaitChild {
    type Output = io::Result<ExitStatus>;

    #[instrument(name = "WaitChild::poll", skip_all)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        debug!("start.");
        unsafe {
            let ev = WaitForSingleObject(*self.0, 0);
            debug!("ev: {ev:?}");
            if ev == WAIT_OBJECT_0 {
                let mut code = 0;
                match GetExitCodeProcess(*self.0, &mut code) {
                    Ok(()) => {
                        debug!("process existed with code {code:?}");
                        let status = ExitStatus::from_raw(code);
                        Poll::Ready(Ok(status))
                    }
                    Err(err) => {
                        let code = err.code();
                        // todo: how does code.0 work?
                        let err = io::Error::from_raw_os_error(code.0);
                        Poll::Ready(Err(err))
                    }
                }
            } else {
                // the process is still running. spawn a thread to wake up the waker when
                // something has changed.
                let waker = cx.waker().clone();
                let wc = self.clone();
                thread::spawn(move || wc.wait_wake(waker));
                Poll::Pending
            }
        }
    }
}

/// a facsimile of PROCESS_INFORMATION, but converts the HANDLE for the process and thread into an
/// owned Handle that closes the HANDLE on drop.
struct ProcessInfo {
    process: Handle,
    // unused but we want to still close it on drop
    #[allow(unused)]
    thread: Handle,
    pid: u32,
    // not used yet
    #[allow(unused)]
    tid: u32,
}

impl From<PROCESS_INFORMATION> for ProcessInfo {
    fn from(info: PROCESS_INFORMATION) -> Self {
        let process = Handle::owned(info.hProcess, HandleKind::Process);
        let thread = Handle::owned(info.hThread, HandleKind::Thread);
        Self {
            process,
            thread,
            pid: info.dwProcessId,
            tid: info.dwThreadId,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PipeKind {
    Stdout,
    Stderr,
}

impl PipeKind {
    /// creates two Handle's, one for writing and one for reading. These are used for piped IO.
    /// The handles that are created will be set with the appropriate HandleKind to allow debugging
    /// to be more precise.
    fn create_handles(self) -> Result<(Handle, Handle)> {
        unsafe {
            let mut rx = Handle::owned(HANDLE::default(), HandleKind::PipeRx(self));
            let mut tx = Handle::owned(HANDLE::default(), HandleKind::PipeTx(self));
            CreatePipe(&mut *rx, &mut *tx, None, 0).map_err(Error::CreateReadPipe)?;
            SetHandleInformation(*tx, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT)
                .map_err(Error::CreateReadPipe)?;
            Ok((tx, rx))
        }
    }
}

/// A future that reads from a windows process stdout or stderr handle.
///
/// The future is started using the start() method, supplying the specific handle. This kicks
/// off a new thread to perform synchronous reads from the child process using the windows HANDLE.
/// The read results are then transmitted on the mpsc channel.
///
/// The future implementation simply reads updates from this channel. The background thread and the
/// future use an AtomicWaker to ensure that the future will wake up from a Pending state.
struct WinHandleOutputStream {
    rx: Arc<Mutex<mpsc::Receiver<io::Result<Vec<u8>>>>>,
    buffer: Vec<u8>,
    pos: usize,
    waker: Arc<AtomicWaker>,
}

impl WinHandleOutputStream {
    /// Starts a new output stream. This will start a background thread to read from the child
    /// process pipe. The thread will communicate back to the returned future over an mpsc channel.
    /// This channel is then used to implement AsyncRead.
    fn start(handle: Handle) -> Self {
        let (tx, rx) = mpsc::sync_channel(1024);
        let rx = Arc::new(Mutex::new(rx));
        let position = 0;
        let buffer = Vec::new();
        let waker = Arc::new(AtomicWaker::new());
        let stream = Self {
            rx,
            buffer,
            pos: position,
            waker: waker.clone(),
        };
        std::thread::spawn(move || Self::read_handle(waker, handle, tx));
        stream
    }

    #[instrument(skip_all, name="read_handle", fields(kind = ?handle.kind))]
    fn read_handle(w: Arc<AtomicWaker>, handle: Handle, tx: mpsc::SyncSender<io::Result<Vec<u8>>>) {
        debug!("start.");
        loop {
            debug!("loop enter.");
            let mut buf = [0; 1024];
            let mut bs_read = 0;
            if let Err(err) = unsafe { ReadFile(*handle, Some(&mut buf), Some(&mut bs_read), None) }
            {
                if err == ERROR_BROKEN_PIPE.into() {
                    // TODO: is this normal when using a pipe to read from child stdout/stderr?
                    debug!("Pipe ended. Will break since bs_read is zero.");
                } else {
                    let msg = format!("ReadFile: {err}");
                    debug!("{msg}");
                    let err = io::Error::new(io::ErrorKind::Other, msg);
                    if let Err(err) = tx.send(Err(err)) {
                        warn!("could not send err: {err}");
                    }
                    w.wake();
                    break;
                }
            }
            debug!("ReadFile reports {bs_read} bytes read.");
            let buf = buf.as_slice();
            let bs_read: usize = bs_read.try_into().unwrap_or_else(|err| {
                panic!("could not convert bs_read ({bs_read}) to usize: {err}")
            });
            let buf = Vec::from(&buf[0..bs_read]);
            debug!("sending {} bytes.", buf.len());
            if let Err(err) = tx.send(Ok(buf)) {
                debug!("send failed: {err}. break.");
                break;
            }
            w.wake();
            if bs_read == 0 {
                debug!("EOF. break.");
                break;
            }
        }
    }
}

// Implementing AsyncRead is a matter of reading from the mpsc channel that the background thread
// is producing into.
impl AsyncRead for WinHandleOutputStream {
    #[instrument(name = "WinHandleOutputStream::poll_read", skip_all)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        debug!("START");
        self.waker.register(cx.waker());
        if self.pos >= self.buffer.len() {
            // we are out of data.
            let recv = self.rx.lock().unwrap().try_recv();
            match recv {
                Ok(Ok(bs)) => {
                    debug!("got {} bytes from receiver", self.buffer.len());
                    self.buffer = bs;
                    self.pos = 0;
                }
                Ok(Err(io_err)) => {
                    debug!("got err from receiver: {io_err}");
                    return Poll::Ready(Err(io_err));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    debug!("empty read from receiver");
                    return Poll::Pending;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    debug!("receiver disconnected");
                    return Poll::Ready(Ok(()));
                }
            };
        }
        // if we got here, we either had data or were able to fetch it.

        // avail is the number of bytes we have to put into the buffer.
        let avail = self.buffer.len() - self.pos;
        // to_copy is how many we are able to copy
        let to_copy = std::cmp::min(avail, buf.remaining());

        debug!("copying {to_copy} bytes into readbuf");

        buf.put_slice(&self.buffer[self.pos..self.pos + to_copy]);
        self.pos += to_copy;

        // signal to the caller that the buffer is ready
        Poll::Ready(Ok(()))
    }
}

/// converts a series of tokens to a u16-encoded byte sequence that the windows api appreciates.
struct WinArgs(Vec<u16>);

impl WinArgs {
    fn new<I>(items: I) -> Self
    where
        I: IntoIterator<Item = OsString>,
    {
        use OsStrExt;
        let u16s = items
            .into_iter()
            .enumerate()
            .flat_map(|(i, t)| {
                (i > 0)
                    .then_some(OsString::from(" "))
                    .into_iter()
                    .chain(Some(t))
                    .flat_map(|s| s.encode_wide().collect::<Vec<_>>())
            })
            .chain(Some(0))
            .collect::<Vec<_>>();
        Self(u16s)
    }

    fn as_pwstr(&mut self) -> PWSTR {
        let ptr = self.0.as_mut_ptr();
        PWSTR(ptr)
    }
}

#[cfg(test)]
mod throwaway_tests {
    //! These are throwaway tests that I created while I was exploring how to use the windows
    //! apis around creating processes, and dealing with HANDLE's for reading stdout/stderr.
    //!
    //! Normally I'd throw these away unless they would help others understand how to use those
    //! apis in the future. I'm leaving them here to illustrate how i typically attack problems I
    //! am unfamiliar with.
    //!
    //! The main tests in the coord module exercise the core functionality that the windows
    //! code implements. If there were windows-specific tests I wanted to keep I would put them in
    //! this file in a tests module.
    use super::*;
    use std::{
        ffi::OsStr,
        mem,
        os::windows::ffi::OsStrExt,
        process::Stdio,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::{
        io::{AsyncReadExt, BufReader},
        process::Command,
    };
    use windows::{
        core::PWSTR,
        Win32::{
            Foundation::{CloseHandle, SetHandleInformation, HANDLE_FLAG_INHERIT},
            Storage::FileSystem::ReadFile,
            System::{
                Diagnostics::ToolHelp::{self, TH32CS_SNAPTHREAD},
                JobObjects::{AssignProcessToJobObject, CreateJobObjectW},
                Pipes::CreatePipe,
                Threading::{
                    CreateProcessW, OpenThread, ResumeThread, CREATE_SUSPENDED,
                    PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
                    STARTUPINFOW, THREAD_SUSPEND_RESUME,
                },
            },
        },
    };
    use ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, THREADENTRY32};

    #[derive(thiserror::Error, Debug)]
    pub enum JobObjectError {
        #[error("could not create job object: {0}")]
        New(#[source] windows_result::Error),

        #[error("no raw handle available on child")]
        NoChildRawHandle,

        #[error("assign failed: {0}")]
        Assign(#[source] windows_result::Error),
    }

    struct JobObject {
        handle: HANDLE,
    }

    impl JobObject {
        fn default() -> Result<Self, JobObjectError> {
            let atts = None;
            let core = None;
            let handle = unsafe { CreateJobObjectW(atts, core).map_err(JobObjectError::New)? };
            Ok(Self { handle })
        }

        fn assign(&mut self, child: &tokio::process::Child) -> Result<(), JobObjectError> {
            let child = child.raw_handle().ok_or(JobObjectError::NoChildRawHandle)?;
            let child = HANDLE(child);
            let res = unsafe { AssignProcessToJobObject(self.handle, child) };
            res.map_err(JobObjectError::Assign)
        }
    }

    impl Drop for JobObject {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle).expect("failed to close job object handle");
            }
        }
    }

    struct WinTokens {
        u16s: Vec<u16>,
        pwstr: PWSTR,
    }
    impl std::fmt::Debug for WinTokens {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            unsafe {
                f.debug_struct("CmdArgs")
                    .field("u16s", &self.u16s)
                    .field("pwstr", &self.pwstr)
                    .field("string", &self.pwstr.to_string())
                    .finish()
            }
        }
    }
    impl WinTokens {
        fn from<'a, I>(toks: I) -> Self
        where
            I: IntoIterator<Item = &'a str>,
        {
            let mut u16s = toks
                .into_iter()
                .enumerate()
                .flat_map(|(i, t)| {
                    (i > 0)
                        .then_some(" ")
                        .into_iter()
                        .chain(Some(t))
                        .flat_map(|s| OsStr::new(s).encode_wide())
                })
                .chain(Some(0))
                .collect::<Vec<_>>();
            let pwstr = PWSTR(u16s.as_mut_ptr());
            Self { u16s, pwstr }
        }

        fn pwstr(&self) -> PWSTR {
            self.pwstr
        }
    }

    // start a new process using CreateProcess and read the expected stdout.
    #[tokio::test]
    async fn create_process_stdout() {
        unsafe {
            let mut read_pipe = HANDLE::default();
            let mut write_pipe = HANDLE::default();
            CreatePipe(&mut read_pipe, &mut write_pipe, None, 0).expect("create pipe");
            SetHandleInformation(write_pipe, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT)
                .expect("set handle info");

            let lpcommandline = WinTokens::from(["cmd", "/c", "echo", "hi"]);
            println!("cmd line: {lpcommandline:#?}");
            let lpprocessattributes = None;
            let lpthreadattributes = None;
            let binherithandles = true;
            let dwcreationflags = PROCESS_CREATION_FLAGS::default();
            let lpenvironment = None;
            let lpcurrentdirectory = None;

            let mut si = STARTUPINFOW::default();
            si.cb = mem::size_of::<STARTUPINFOW>() as u32;
            si.hStdOutput = write_pipe;
            si.dwFlags |= STARTF_USESTDHANDLES;

            let mut proc_info = PROCESS_INFORMATION::default();
            CreateProcessW(
                PWSTR::null(),
                lpcommandline.pwstr(),
                lpprocessattributes,
                lpthreadattributes,
                binherithandles,
                dwcreationflags,
                lpenvironment,
                lpcurrentdirectory,
                &mut si,
                &mut proc_info,
            )
            .expect("create process");
            assert!(!proc_info.hProcess.is_invalid());
            assert!(!proc_info.hThread.is_invalid());
            assert!(proc_info.dwProcessId > 0);
            assert!(proc_info.dwThreadId > 0);

            // close the write pipe so that reads can happen
            CloseHandle(write_pipe).expect("close write pipe");

            let read_pipe = Arc::new(Mutex::new(read_pipe));
            let read_pipe = read_pipe.lock().expect("lock read pipe");

            // read stdout
            println!("Trying to read stdout");
            let mut buffer = [0; 1024];
            let mut bytes_read = 0;
            ReadFile(*read_pipe, Some(&mut buffer), Some(&mut bytes_read), None)
                .expect("read file");
            println!("Read {bytes_read} bytes");
            assert!(bytes_read > 0);
            let out = std::str::from_utf8(&buffer.as_slice()[0..(bytes_read as usize)]).unwrap();
            assert_eq!(out, "hi\r\n");

            CloseHandle(proc_info.hProcess).expect("close proc");
            CloseHandle(proc_info.hThread).expect("close thread");
        }
    }

    // start a new process using CreateProcess
    #[tokio::test]
    async fn create_process() {
        unsafe {
            let lpcommandline = WinTokens::from(["echo", "hi"]);
            println!("cmd line: {lpcommandline:#?}");
            let lpprocessattributes = None;
            let lpthreadattributes = None;
            let binherithandles = true;
            let dwcreationflags = PROCESS_CREATION_FLAGS::default();
            let lpenvironment = None;
            let lpcurrentdirectory = None;
            let lpstartupinfo = STARTUPINFOW::default();
            let mut proc_info = PROCESS_INFORMATION::default();
            let res = CreateProcessW(
                PWSTR::null(),
                lpcommandline.pwstr(),
                lpprocessattributes,
                lpthreadattributes,
                binherithandles,
                dwcreationflags,
                lpenvironment,
                lpcurrentdirectory,
                &lpstartupinfo,
                &mut proc_info,
            );
            res.expect("create process");
            assert!(!proc_info.hProcess.is_invalid());
            assert!(!proc_info.hThread.is_invalid());
            assert!(proc_info.dwProcessId > 0);
            assert!(proc_info.dwThreadId > 0);
            CloseHandle(proc_info.hProcess).expect("close proc");
            CloseHandle(proc_info.hThread).expect("close thread");
        }
    }

    // test that a process can be started using the tokio apis in the suspended state, and the
    // resumed via the win32 apis. this is not ideal since  we have to jump through hoops to get
    // the thread id for the suspended process. if we're going to suspend, we probably will want to
    // use CreateProcess instead and use the PROCESS_INFORMATION output variable to get the thread
    // id.
    #[tokio::test]
    async fn spawn_suspend_resume() {
        unsafe {
            let mut child = Command::new("echo")
                .creation_flags(CREATE_SUSPENDED.0)
                .kill_on_drop(true)
                .arg("hi")
                .stdout(Stdio::piped())
                .spawn()
                .expect("spawn failed");
            let pid = child.id().expect("no process id");
            assert!(pid > 0);
            println!("goal is to find pid: {pid}");
            let flags = TH32CS_SNAPTHREAD;
            let th32_process_id = 0;
            let h_threadsnap = CreateToolhelp32Snapshot(flags, th32_process_id).expect("snapshot");
            let mut te = THREADENTRY32::default();
            te.dwSize = mem::size_of::<THREADENTRY32>() as u32;
            Thread32First(h_threadsnap, &mut te).expect("thread32 first");
            let mut tid: Option<u32> = None;
            loop {
                let th_32_pid = te.th32OwnerProcessID;
                let th_32_tid = te.th32ThreadID;
                if pid == th_32_pid {
                    println!("th32 pid={th_32_pid} tid={th_32_tid}");
                    tid.replace(th_32_tid);
                    break;
                }
                let res = Thread32Next(h_threadsnap, &mut te);
                if res.is_err() {
                    break;
                }
            }
            let tid = tid.expect("no thread id");
            let thread = OpenThread(THREAD_SUSPEND_RESUME, false, tid).expect("open thread");
            let res = ResumeThread(thread);
            assert!(res > 0, "thread was not resumed");
            CloseHandle(thread).expect("close thread handle");

            // verify stdout
            let stdout = child.stdout.take().expect("no stdout");
            let mut stdout = BufReader::new(stdout);
            let mut buf = String::new();
            stdout
                .read_to_string(&mut buf)
                .await
                .expect("read to string");
            assert_eq!(buf.trim(), "hi");

            // assert job exit code is ok
            let status = child.wait().await.expect("wait failed");
            assert!(status.success());
        }
    }

    // verify that spawning a new process in a suspended state does not let the process make
    // progress.
    #[tokio::test]
    async fn spawn_suspend() {
        let mut child = tokio::time::timeout(Duration::from_secs(1), async {
            // create the child in a suspended state
            let child = Command::new("echo")
                .creation_flags(CREATE_SUSPENDED.0)
                .kill_on_drop(true)
                .arg("hi")
                .stdout(Stdio::piped())
                .spawn()
                .expect("spawn failed");
            let pid = child.id().expect("no process id");
            assert!(pid > 0);
            child
        })
        .await
        .expect("timeout");

        tokio::time::timeout(Duration::from_millis(250), async move {
            let mut stdout = child.stdout.take().expect("no stdout");
            let mut buf = Vec::with_capacity(1024);
            stdout.read(&mut buf[..]).await.expect("read failed");
        })
        .await
        .unwrap_err(); // require timeout
    }

    // a silly test which spawns a new process and then attaches it to a job handle. this is not
    // what we ultimately want because of the race between the process starting and the job object
    // being later attached.
    #[tokio::test]
    async fn spawn_attach() {
        let mut job = JobObject::default().unwrap();

        // echo hi to stdout
        let mut cmd = Command::new("echo");
        cmd.arg("hi");
        cmd.stdout(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn failed");

        // assign process to job
        job.assign(&child).expect("assign");

        // verify stdout
        let stdout = child.stdout.take().expect("no stdout");
        let mut stdout = BufReader::new(stdout);
        let mut buf = String::new();
        stdout
            .read_to_string(&mut buf)
            .await
            .expect("read to string");
        assert_eq!(buf.trim(), "hi");

        // assert job exit code is ok
        let status = child.wait().await.expect("wait failed");
        assert!(status.success());
    }

    #[tokio::test]
    async fn echohi() {
        let mut cmd = Command::new("echo");
        cmd.arg("hi");
        cmd.stdout(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn failed");
        let stdout = child.stdout.take().expect("no stdout");
        let mut stdout = BufReader::new(stdout);
        let mut buf = String::new();
        stdout
            .read_to_string(&mut buf)
            .await
            .expect("read to string");
        assert_eq!(buf.trim(), "hi");
    }
}
