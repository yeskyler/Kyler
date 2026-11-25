mod adapter;
mod adapters;
mod bindings;
mod catalog;
mod message;
mod plain_text;
mod store;

pub use adapter::{Adapter, StreamStopped, Webhook};
pub use bindings::Binding;
pub use catalog::{CHANNELS, ChannelDefinition, Settings, WEB_ORIGIN_KEY};
pub use message::{Incoming, IncomingAttachment, OutgoingFile};
pub use store::{ChannelStore, Job, Reply, ReplyReceipt};

pub use adapters::qqbot::files::download as download_qq_attachment;
