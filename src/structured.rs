//! In-process presentation harness. These values are not RPC or MCP envelopes.

mod binding;
mod context;
mod guarded_view;
mod payload;
mod serde_view;

pub use binding::StructuredBinding;
pub use context::StructuredContext;
pub use payload::{Payload, Transcript};

pub(crate) use context::{LiveSink, SendFuture};
pub(crate) use guarded_view::GuardedView;
