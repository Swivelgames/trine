//! Wire primitives shared by the stdio protocols.

pub(crate) mod framing;
mod id;
pub mod stdio;

pub(crate) use id::Id;
