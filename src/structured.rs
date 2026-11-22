//! Structured views and result bindings for in-process calls and live RPC and MCP hosts.

mod binding;
mod context;
mod guarded_view;
mod payload;
mod serde_view;

pub use binding::StructuredBinding;
pub use context::StructuredContext;
pub use payload::{Payload, Transcript};

pub(crate) use context::{Destination, LiveSink, ProgressSink, SendFuture};
pub(crate) use guarded_view::GuardedView;
