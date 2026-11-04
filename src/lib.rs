//! Experimental typed actions, invocation contexts, and schema-assisted CLI views.
//!
//! Actions expose standard futures and remain independent of their presentation.

#![doc = include_str!("../README.md")]

pub mod action;
pub mod application;
pub mod cli;
pub mod diagnostics;
pub mod error;
pub mod execution;
pub mod invocation;
pub mod native_path;
pub mod registry;
pub mod rpc;
pub mod structured;
pub mod view;

pub use action::{Action, RegistrationError};
pub use application::{Application, Entry};
pub use invocation::{Cancellation, EmitError, InvocationContext};
pub use native_path::NativePath;
