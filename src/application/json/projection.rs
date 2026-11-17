//! Typed result encoding and completion policy shared by both CLI presentations.
use super::{capture_view::CaptureView, policy_view::PolicyView};
use crate::error::{Failure, FailureKind};
use serde::Serialize;
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub(super) type ExitPolicy<O, E> = dyn Fn(&Result<O, E>) -> u8 + Send + Sync;
type Encoder<O, E> = dyn Fn(&Result<O, E>) -> Result<Value, Failure> + Send + Sync;

pub(crate) struct JsonProjection<O, E> {
	pub(super) exit: Arc<ExitPolicy<O, E>>,
	pub(super) encode: Arc<Encoder<O, E>>,
}

impl<O, E> Clone for JsonProjection<O, E> {
	fn clone(&self) -> Self {
		Self {
			exit: self.exit.clone(),
			encode: self.encode.clone(),
		}
	}
}

impl<O: Serialize, E: Serialize> JsonProjection<O, E> {
	pub(crate) fn new(exit: impl Fn(&Result<O, E>) -> u8 + Send + Sync + 'static) -> Self {
		Self {
			exit: Arc::new(exit),
			encode: Arc::new(|outcome| {
				match outcome {
					Ok(value) => serde_json::to_value(value),
					Err(value) => serde_json::to_value(value),
				}
				.map_err(|error| Failure::new(FailureKind::EncodingFailed, error))
			}),
		}
	}
}

impl<O, E> JsonProjection<O, E> {
	pub(crate) fn human<V>(&self, view: V) -> PolicyView<V, O, E> {
		PolicyView {
			view,
			exit: self.exit.clone(),
		}
	}

	pub(crate) fn capture(&self, result: Arc<Mutex<Option<Value>>>) -> CaptureView<O, E> {
		CaptureView {
			projection: self.clone(),
			result,
		}
	}
}
