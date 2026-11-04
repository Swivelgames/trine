//! Default serialization of typed results and optional progress.

use super::{Payload, StructuredContext};
use crate::{
	error::{Failure, FailureKind},
	view::View,
};
use serde::Serialize;
use serde_json::Value;
use std::{marker::PhantomData, sync::Arc};

pub(super) type Mapper<P> = dyn Fn(P) -> Result<Value, Failure> + Send + Sync;

pub(super) struct SerdeView<O, E, P> {
	context: StructuredContext,
	mapper: Option<Arc<Mapper<P>>>,
	marker: PhantomData<fn() -> (O, E)>,
}

impl<O, E, P> SerdeView<O, E, P> {
	pub(super) fn new(context: StructuredContext, mapper: Option<Arc<Mapper<P>>>) -> Self {
		Self {
			context,
			mapper,
			marker: PhantomData,
		}
	}
}

impl<O: Serialize + Sync, E: Serialize + Sync, P: Send + 'static> View<O, E, P>
	for SerdeView<O, E, P>
{
	type Completion = ();
	type Error = Failure;

	async fn progress(&mut self, event: P) -> Result<(), Failure> {
		if let Some(mapper) = &self.mapper {
			self.context.progress(mapper(event)?).await?;
		}
		Ok(())
	}

	async fn finish(&mut self, outcome: &Result<O, E>) -> Result<(), Failure> {
		let payload = match outcome {
			Ok(value) => Payload::Success(
				serde_json::to_value(value)
					.map_err(|e| Failure::new(FailureKind::EncodingFailed, e))?,
			),
			Err(error) => Payload::DomainFailure(
				serde_json::to_value(error)
					.map_err(|e| Failure::new(FailureKind::EncodingFailed, e))?,
			),
		};

		self.context.terminal(payload).await
	}
}
