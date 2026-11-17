//! Encode a terminal outcome without writing until execution settles.
use super::projection::JsonProjection;
use crate::{application::CliCompletion, error::Failure, view::View};
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub(crate) struct CaptureView<O, E> {
	pub(super) projection: JsonProjection<O, E>,
	pub(super) result: Arc<Mutex<Option<Value>>>,
}

impl<O: Sync, E: Sync, P: Send> View<O, E, P> for CaptureView<O, E> {
	type Completion = CliCompletion;
	type Error = Failure;

	async fn progress(&mut self, _: P) -> Result<(), Failure> {
		Ok(())
	}

	async fn finish(&mut self, outcome: &Result<O, E>) -> Result<CliCompletion, Failure> {
		let value = (self.projection.encode)(outcome)?;
		*self.result.lock().expect("JSON result lock") = Some(value);
		Ok(CliCompletion::exit((self.projection.exit)(outcome)))
	}
}
