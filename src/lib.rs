#![warn(clippy::pedantic)]
#![allow(clippy::similar_names)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::needless_pass_by_value)]

pub mod grpc;
// we have limited control of the generated code, so don't let clippy loose on it
#[allow(clippy::pedantic)]
pub mod pb;
pub mod proc;
pub mod utils;
