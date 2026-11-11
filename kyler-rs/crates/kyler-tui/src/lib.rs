//! Terminal UI backed by an agent running in this process.
mod agent;
mod app;
mod attachments;
mod commands;
mod components;
mod effects;
mod input;
mod layout;
mod questions;
mod state;
mod terminal;
#[cfg(test)]
mod tests;
mod update;

pub use app::run;
