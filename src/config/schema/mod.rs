//! The config schema: the Rust types a `Procfile.toml` deserializes into.
//! [`process`] holds a process entry and its release policy; [`readiness`] holds
//! the readiness check and its hand-written deserializers.

mod process;
mod readiness;

pub use process::{ExitCodes, Process, ReadyWhen};
pub use readiness::{Readiness, StatusCodes};
