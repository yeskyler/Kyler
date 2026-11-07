//! Append-only, committed execution history. Replay never executes models or tools.
pub mod reader;
pub mod record;
pub mod replay;
pub mod writer;

pub use reader::RolloutReader;
pub use record::{RolloutPayload, RolloutPosition, RolloutRecord};
pub use writer::RolloutWriter;
