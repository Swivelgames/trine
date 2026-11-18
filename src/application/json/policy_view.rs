//! Apply action-owned CLI exit policy after human presentation succeeds.
use super::projection::ExitPolicy;
use crate::{Cancellation, application::CliCompletion, invocation::Confirmation, view::View};
use std::sync::Arc;

pub(crate) struct PolicyView<V, O, E> {
	pub(super) view: V,
	pub(super) exit: Arc<ExitPolicy<O, E>>,
}

impl<O: Sync, E: Sync, P: Send, V: View<O, E, P, Completion = ()>> View<O, E, P>
	for PolicyView<V, O, E>
{
	type Completion = CliCompletion;
	type Error = V::Error;

	async fn confirm(
		&mut self,
		request: Confirmation,
		cancellation: Cancellation,
	) -> Result<Option<bool>, Self::Error> {
		self.view.confirm(request, cancellation).await
	}

	async fn select(
		&mut self,
		request: crate::invocation::Selection,
		cancellation: Cancellation,
	) -> Result<Option<usize>, Self::Error> {
		self.view.select(request, cancellation).await
	}

	async fn progress(&mut self, event: P) -> Result<(), Self::Error> {
		self.view.progress(event).await
	}

	async fn finish(&mut self, outcome: &Result<O, E>) -> Result<CliCompletion, Self::Error> {
		self.view.finish(outcome).await?;
		Ok(CliCompletion::exit((self.exit)(outcome)))
	}
}
