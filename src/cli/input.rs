//! Normalized CLI fields, with native paths kept outside JSON values.

mod deserialize;

use super::CliError;
use crate::NativePath;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub(super) enum FieldValue {
	Scalar(Value),
	RepeatedString(String),
	Path(NativePath),
}

#[derive(Debug, Default)]
pub struct InputFields {
	pub scalars: Map<String, Value>,
	pub paths: BTreeMap<String, NativePath>,
	pub groups: BTreeMap<String, Vec<InputFields>>,
}

impl InputFields {
	pub fn take_path(&mut self, field: &str) -> Option<NativePath> {
		self.paths.remove(field)
	}

	pub fn take_group(&mut self, field: &str) -> Vec<Self> {
		self.groups.remove(field).unwrap_or_default()
	}

	/// Construct the input type directly, including native paths and ordered groups.
	pub fn deserialize<T: DeserializeOwned>(self) -> Result<T, CliError> {
		T::deserialize(deserialize::InputValue::Object(self))
			.map_err(|error| CliError(format!("invalid action input: {error}")))
	}
}
