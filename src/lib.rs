//! proccie is a process manager that reads a TOML config and orchestrates child
//! processes with dependency ordering, readiness, retries, and graceful shutdown.

pub mod config;
pub mod mux;
pub mod runner;
