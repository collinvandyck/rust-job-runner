use anyhow::Result;
use futures::Future;
use std::{io, pin::Pin, process::ExitStatus};
use tokio::{
    io::AsyncRead,
    process::{ChildStderr, ChildStdout},
};

use super::types::JobSpec;

cfg_if::cfg_if! {
    if #[cfg(target_os = "windows")] {
        mod win;
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to spawn child: {0}")]
    Spawn(#[source] io::Error),

    #[error("spawned child had no pid")]
    NoPid,

    #[cfg(target_os = "windows")]
    #[error(transparent)]
    Win(win::Error),
}

type WaitReturn<'a> = Pin<Box<dyn Future<Output = io::Result<ExitStatus>> + Send + 'a>>;
type KillReturn<'a> = Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>>;
pub trait OutputStream: AsyncRead + Unpin + Send {}
impl<T: AsyncRead + Unpin + Send> OutputStream for T {}

/// represents a spawned child process.
pub trait Child: Send {
    fn pid(&self) -> u32;
    fn stdout(&mut self) -> Box<dyn OutputStream>;
    fn stderr(&mut self) -> Box<dyn OutputStream>;
    fn wait(&mut self) -> WaitReturn<'_>;
    fn kill(&mut self) -> KillReturn<'_>;
}

/// a handle to a child process that has been spawned. allows the caller to not be tightly coupled
/// to the tokio::process APIs in the event we need to have some flexibility wrt job control on
/// different platforms
pub struct TokioChild {
    child: tokio::process::Child,
    pid: u32,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}

// the Child impl for TokioChild is pretty simple. we just delegate to the library apis
impl Child for TokioChild {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn stdout(&mut self) -> Box<dyn OutputStream> {
        Box::new(self.stdout.take().expect("no stdout on child"))
    }

    fn stderr(&mut self) -> Box<dyn OutputStream> {
        Box::new(self.stderr.take().expect("no stderr on child"))
    }

    fn wait(&mut self) -> WaitReturn<'_> {
        Box::pin(async move { self.child.wait().await })
    }

    fn kill(&mut self) -> KillReturn<'_> {
        Box::pin(async move { self.child.kill().await })
    }
}

#[allow(unused)]
pub struct Opts {
    spec: JobSpec,
    limits: Option<Limits>,
}

impl Opts {
    pub fn new(spec: JobSpec) -> Self {
        Self { spec, limits: None }
    }

    pub fn spawn(self) -> Result<Box<dyn Child>, Error> {
        cfg_if::cfg_if! {
            if #[cfg(target_os = "windows")] {
                let child = win::Child::spawn(self).map_err(Error::Win)?;
                Ok(Box::new(child))
            } else {
                use std::process::Stdio;
                let mut cmd = tokio::process::Command::new(self.spec.exe);
                cmd.args(self.spec.args);
                cmd.kill_on_drop(true);
                cmd.stdin(Stdio::null());
                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::piped());
                let mut child = cmd.spawn().map_err(Error::Spawn)?;
                let pid = child.id().ok_or(Error::NoPid)?;
                let stdout = child.stdout.take().expect("stdout must be piped");
                let stderr = child.stderr.take().expect("stderr must be piped");
                let child = TokioChild {
                    child,
                    pid,
                    stdout: Some(stdout),
                    stderr: Some(stderr),
                };
                Ok(Box::new(child))
            }
        }
    }
}

pub struct Limits {}
