//! proccie is a process manager that reads a TOML config and orchestrates child
//! processes with dependency ordering, readiness, retries, and graceful shutdown.

pub mod config;
pub mod logger;
pub mod runner;
pub mod service;
mod sync;
pub mod tui;
