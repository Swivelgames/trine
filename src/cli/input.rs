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
	RepeatedPath(NativePath),
}

/// Parsed CLI values grouped by representation for custom typed input assembly.
#[derive(Debug, Default)]
pub struct InputFields {
	/// JSON-compatible scalars and repeated strings keyed by schema field name.
	pub scalars: Map<String, Value>,
	/// Single native paths keyed by field name.
	pub paths: BTreeMap<String, NativePath>,
	/// Repeated native paths in input order, keyed by field name.
	pub path_lists: BTreeMap<String, Vec<NativePath>>,
	/// Repeated object rows grouped under their array field name.
	pub groups: BTreeMap<String, Vec<InputFields>>,
}

impl InputFields {
	/// Remove and return the native path mapped to this field.
	pub fn take_path(&mut self, field: &str) -> Option<NativePath> {
		self.paths.remove(field)
	}

	/// Remove a group's ordered rows, returning an empty vector when absent.
	pub fn take_group(&mut self, field: &str) -> Vec<Self> {
		self.groups.remove(field).unwrap_or_default()
	}

	/// Construct the input type directly, including native paths and ordered groups.
	pub fn deserialize<T: DeserializeOwned>(self) -> Result<T, CliError> {
		T::deserialize(deserialize::InputValue::Object(self))
			.map_err(|error| CliError(format!("invalid action input: {error}")))
	}
}
