//! Structured views and result bindings for in-process calls and live RPC and MCP hosts.
//!
//! Use [`StructuredBinding::serde`] to serialize a handler's returned value.
//! For an in-process invocation, inspect [`Transcript::terminal`] alongside
//! [`StructuredReport::execution`](crate::application::StructuredReport::execution).
//! Live hosts translate the same payload into their protocol's response.
//! The [results and failures guide](crate::guides::results_and_failures) follows a
//! domain rejection through both protocols and shows how to diagnose a failed write.

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
