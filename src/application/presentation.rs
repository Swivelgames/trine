//! Invocation-scoped CLI facilities and type-erased views after typed assembly.

use crate::{
	error::{Failure, FailureKind},
	view::View,
};
use std::{
	future::Future,
	io::{self, Write},
	pin::Pin,
	sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Output(Arc<Mutex<Box<dyn Write + Send>>>);

impl Output {
	pub fn new(writer: impl Write + Send + 'static) -> Self {
		Self(Arc::new(Mutex::new(Box::new(writer))))
	}

	/// Writes and eagerly flushes under one lock, surfacing buffered output failures.
	pub fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
		let mut writer = self
			.0
			.lock()
			.map_err(|_| io::Error::other("output lock poisoned"))?;
		writer.write_all(bytes)?;
		writer.flush()
	}
}

#[derive(Clone)]
pub struct CliViewContext {
	pub stdout: Output,
	pub stderr: Output,
}

impl Default for CliViewContext {
	fn default() -> Self {
		Self {
			stdout: Output::new(io::stdout()),
			stderr: Output::new(io::stderr()),
		}
	}
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CliCompletion {
	pub exit_code: Option<u8>,
}

impl CliCompletion {
	pub fn exit(code: u8) -> Self {
		Self {
			exit_code: Some(code),
		}
	}
}

impl From<()> for CliCompletion {
	fn from(_: ()) -> Self {
		Self::default()
	}
}

type ViewFuture<'a, C = ()> = Pin<Box<dyn Future<Output = Result<C, Failure>> + Send + 'a>>;

trait DynView<O, E, P, C>: Send {
	fn progress(&mut self, event: P) -> ViewFuture<'_>;

	fn finish<'a>(&'a mut self, outcome: &'a Result<O, E>) -> ViewFuture<'a, C>;
}

struct TypedView<V>(V);

impl<O: Sync, E: Sync, P: Send + 'static, C: Send, V: View<O, E, P>> DynView<O, E, P, C>
	for TypedView<V>
where
	V::Completion: Into<C>,
	V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
	fn progress(&mut self, event: P) -> ViewFuture<'_> {
		Box::pin(async move {
			self.0
				.progress(event)
				.await
				.map_err(|e| Failure::from_box(FailureKind::OutputFailed, e.into()))
		})
	}

	fn finish<'a>(&'a mut self, outcome: &'a Result<O, E>) -> ViewFuture<'a, C> {
		Box::pin(async move {
			self.0
				.finish(outcome)
				.await
				.map(Into::into)
				.map_err(|e| Failure::from_box(FailureKind::OutputFailed, e.into()))
		})
	}
}

pub(crate) struct ErasedView<O, E, P, C = ()>(Box<dyn DynView<O, E, P, C>>);

impl<O: Sync, E: Sync, P: Send + 'static, C: Send> ErasedView<O, E, P, C> {
	pub(crate) fn new<V: View<O, E, P> + 'static>(view: V) -> Self
	where
		V::Completion: Into<C>,
		V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
	{
		Self(Box::new(TypedView(view)))
	}
}

impl<O: Sync, E: Sync, P: Send + 'static, C: Send> View<O, E, P> for ErasedView<O, E, P, C> {
	type Completion = C;
	type Error = Failure;

	async fn progress(&mut self, event: P) -> Result<(), Failure> {
		self.0.progress(event).await
	}

	async fn finish(&mut self, outcome: &Result<O, E>) -> Result<C, Failure> {
		self.0.finish(outcome).await
	}
}
