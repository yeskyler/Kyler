//! Single-column terminal client. Sessions, tools and sandbox authority stay on the server.
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
