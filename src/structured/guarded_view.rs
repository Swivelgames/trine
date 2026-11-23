//! Enforce structured terminal delivery around a custom view.

use super::StructuredContext;
use crate::{
	error::Failure,
	view::{ErasedView, View},
};

pub(crate) struct GuardedView<O, E, P> {
	pub view: ErasedView<O, E, P>,
	pub context: StructuredContext,
}

impl<O: Sync, E: Sync, P: Send + 'static> View<O, E, P> for GuardedView<O, E, P> {
	type Completion = ();
	type Error = Failure;

	async fn start(&mut self) -> Result<(), Failure> {
		self.view.start().await
	}

	async fn progress(&mut self, event: P) -> Result<(), Failure> {
		self.view.progress(event).await
	}

	async fn finish(&mut self, outcome: &Result<O, E>) -> Result<(), Failure> {
		self.context.begin_finish();
		self.view.finish(outcome).await?;
		self.context.close(true)
	}
}
