//! Preserve typed view behavior behind the application dispatch boundary.

use super::View;
use crate::error::{Failure, FailureKind};
use std::{future::Future, pin::Pin};

type ViewFuture<'a, C = ()> = Pin<Box<dyn Future<Output = Result<C, Failure>> + Send + 'a>>;

trait DynView<O, E, P, C>: Send {
	fn start(&mut self) -> ViewFuture<'_>;
	fn progress(&mut self, event: P) -> ViewFuture<'_>;
	fn confirm(
		&mut self,
		request: crate::invocation::Confirmation,
		cancellation: crate::Cancellation,
	) -> ViewFuture<'_, Option<bool>>;

	fn select(
		&mut self,
		request: crate::invocation::Selection,
		cancellation: crate::Cancellation,
	) -> ViewFuture<'_, Option<usize>>;

	fn finish<'a>(&'a mut self, outcome: &'a Result<O, E>) -> ViewFuture<'a, C>;
}

struct TypedView<V>(V);

impl<O: Sync, E: Sync, P: Send + 'static, C: Send, V: View<O, E, P>> DynView<O, E, P, C>
	for TypedView<V>
where
	V::Completion: Into<C>,
	V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
	fn confirm(
		&mut self,
		request: crate::invocation::Confirmation,
		cancellation: crate::Cancellation,
	) -> ViewFuture<'_, Option<bool>> {
		Box::pin(async move {
			self.0
				.confirm(request, cancellation)
				.await
				.map_err(|e| Failure::from_box(FailureKind::OutputFailed, e.into()))
		})
	}

	fn select(
		&mut self,
		request: crate::invocation::Selection,
		cancellation: crate::Cancellation,
	) -> ViewFuture<'_, Option<usize>> {
		Box::pin(async move {
			self.0
				.select(request, cancellation)
				.await
				.map_err(|e| Failure::from_box(FailureKind::OutputFailed, e.into()))
		})
	}

	fn start(&mut self) -> ViewFuture<'_> {
		Box::pin(async move {
			self.0
				.start()
				.await
				.map_err(|e| Failure::from_box(FailureKind::OutputFailed, e.into()))
		})
	}

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

	async fn confirm(
		&mut self,
		request: crate::invocation::Confirmation,
		cancellation: crate::Cancellation,
	) -> Result<Option<bool>, Failure> {
		self.0.confirm(request, cancellation).await
	}

	async fn select(
		&mut self,
		request: crate::invocation::Selection,
		cancellation: crate::Cancellation,
	) -> Result<Option<usize>, Failure> {
		self.0.select(request, cancellation).await
	}

	async fn start(&mut self) -> Result<(), Failure> {
		self.0.start().await
	}

	async fn progress(&mut self, event: P) -> Result<(), Failure> {
		self.0.progress(event).await
	}

	async fn finish(&mut self, outcome: &Result<O, E>) -> Result<C, Failure> {
		self.0.finish(outcome).await
	}
}
