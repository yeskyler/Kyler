//! SQLite infrastructure and typed application indexes, independent of execution.
pub mod catalog;
mod database;
pub mod gateway;

pub use database::Database;
