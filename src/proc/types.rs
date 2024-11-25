//! a place for our value types to give other modules better S:N

use chrono::{DateTime, Utc};
use std::{ffi::OsString, fmt, ops::Deref};

/// general metadata about any operation on the coordinator
#[derive(Debug, Clone)]
pub struct Ctx {
    pub user: User,
}

impl fmt::Display for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.user)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: String,
}

impl From<&str> for User {
    fn from(value: &str) -> Self {
        Self {
            id: value.to_string(),
        }
    }
}

impl fmt::Display for User {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id)
    }
}

#[derive(Debug, Clone)]
#[must_use]
pub struct JobSpec {
    pub exe: OsString,
    pub args: Vec<OsString>,
}

impl JobSpec {
    pub fn new(exe: impl Into<OsString>) -> Self {
        Self {
            exe: exe.into(),
            args: vec![],
        }
    }

    pub fn exe_and_args(&self) -> Vec<OsString> {
        Some(&self.exe)
            .into_iter()
            .chain(&self.args)
            .map(ToOwned::to_owned)
            .collect()
    }

    pub fn arg(self, arg: impl Into<OsString>) -> Self {
        self.args([arg])
    }

    pub fn args<I, T>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        for arg in args {
            self.args.push(arg.into());
        }
        self
    }
}

/// the primary view of job state, excluding output streams.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub user: User,
    pub killed: Option<JobKilled>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub state: JobState,
}

impl Job {
    /// creates a new job in the started state
    pub fn started(job_id: &JobId, user: &User, pid: &Pid) -> Self {
        Self {
            id: job_id.clone(),
            user: user.clone(),
            killed: None,
            started_at: Utc::now(),
            ended_at: None,
            state: JobState::Running { pid: pid.clone() },
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct JobId(String);

impl From<String> for JobId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for JobId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for JobId {
    type Target = String;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for JobId {
    fn default() -> Self {
        let uuid = uuid::Uuid::now_v7();
        Self(uuid.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Running { pid: Pid },
    Exited { status: ExitStatus },
}

impl JobState {
    pub fn exited(status: impl Into<ExitStatus>) -> Self {
        Self::Exited {
            status: status.into(),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(&self, &JobState::Running { .. })
    }

    pub fn is_exited(&self) -> bool {
        matches!(&self, &JobState::Exited { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pid {
    pub pid: u32,
}

impl From<u32> for Pid {
    fn from(value: u32) -> Self {
        Self { pid: value }
    }
}

impl Pid {
    pub fn new(pid: u32) -> Self {
        Self { pid }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStatus {
    pub code: Option<i32>,
}

impl From<i32> for ExitStatus {
    fn from(value: i32) -> Self {
        Self { code: Some(value) }
    }
}

impl From<std::process::ExitStatus> for ExitStatus {
    fn from(es: std::process::ExitStatus) -> Self {
        Self { code: es.code() }
    }
}

#[derive(Debug, Clone)]
pub struct JobKilled {
    pub killed_at: DateTime<Utc>,
    pub user: User,
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn job_ids_are_unique() {
        let mut ids = HashSet::new();
        for _ in 0..1000 {
            let id = JobId::default();
            assert!(ids.insert(id.clone()), "duplicate job id: {id}");
        }
    }
}
