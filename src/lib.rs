// Library entry point – re-exports all modules so integration tests
// in tests/ can access them via `updock::...`.
pub mod compose;
pub mod config;
pub mod daemon;
pub mod harbor;
pub mod matcher;
pub mod metrics;
pub mod notify;
pub mod server;
pub mod swap;
pub mod watcher;
