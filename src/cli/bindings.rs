//! Explicit argument bindings and registration validation.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::schema::{Scalar, Shape};
use crate::{RegistrationError, action::validate_name};

#[derive(Clone, Debug)]
pub(super) struct Binding {
	pub(super) field: String,
	pub(super) long: Option<String>,
}

/// Explicit CLI spellings for every input field. Group child fields use
/// `repositories[].remote` notation; exactly one group anchor starts each row.
#[derive(Clone, Debug, Default)]
pub struct Bindings {
	pub(super) fields: Vec<Binding>,
	pub(super) group: Option<(String, String)>,
	pub(super) native_paths: BTreeSet<String>,
}

impl Bindings {
	pub fn new() -> Self {
		Self::default()
	}

	/// Explicitly opt a schema path into native OS argument parsing.
	pub fn native_path(mut self, field: impl Into<String>) -> Self {
		self.native_paths.insert(field.into());
		self
	}

	pub fn positional(mut self, field: impl Into<String>) -> Self {
		self.fields.push(Binding {
			field: field.into(),
			long: None,
		});
		self
	}

	pub fn option(mut self, field: impl Into<String>, long: impl Into<String>) -> Self {
		self.fields.push(Binding {
			field: field.into(),
			long: Some(long.into()),
		});
		self
	}

	pub fn group(
		mut self,
		field: impl Into<String>,
		anchor: impl Into<String>,
		long: impl Into<String>,
	) -> Self {
		let field = field.into();
		let anchor = anchor.into();
		self.fields.push(Binding {
			field: format!("{field}[].{anchor}"),
			long: Some(long.into()),
		});
		self.group = Some((field, anchor));
		self
	}
}

/// A second command name for the same handler, with explicit scalar assignments.
#[derive(Clone, Debug)]
pub struct Alias {
	pub(super) name: String,
	pub(super) assignments: Vec<(String, Value)>,
}

impl Alias {
	pub fn new(name: impl Into<String>) -> Self {
		Self {
			name: name.into(),
			assignments: Vec::new(),
		}
	}

	pub fn set(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
		self.assignments.push((field.into(), value.into()));
		self
	}
}

pub(super) fn validate_bindings(
	bindings: &Bindings,
	shape: &Shape,
	fields: &BTreeMap<String, Scalar>,
) -> Result<(), RegistrationError> {
	for (field, scalar) in fields {
		if (*scalar == Scalar::NativePath) != bindings.native_paths.contains(field) {
			return Err(RegistrationError(format!(
				"native-path binding must match native-path schema for '{field}'"
			)));
		}
	}
	for field in &bindings.native_paths {
		if !fields.contains_key(field) {
			return Err(RegistrationError(format!(
				"unknown native-path field '{field}'"
			)));
		}
	}

	let mut bound = BTreeSet::new();
	let mut options = BTreeSet::from(["help", "version"]);
	let mut positionals = 0;

	for binding in &bindings.fields {
		let Some(scalar) = fields.get(&binding.field) else {
			return Err(RegistrationError(format!(
				"binding references unknown field '{}'",
				binding.field
			)));
		};
		if !bound.insert(binding.field.as_str()) {
			return Err(RegistrationError(format!(
				"duplicate binding for '{}'",
				binding.field
			)));
		}

		if let Some(long) = &binding.long {
			validate_name(long)?;
			if !options.insert(long) {
				return Err(RegistrationError(format!(
					"duplicate or reserved option '--{long}'"
				)));
			}
		} else {
			positionals += 1;
			if !matches!(scalar, Scalar::String | Scalar::NativePath)
				|| binding.field.contains("[].")
				|| positionals > 1
			{
				return Err(RegistrationError(format!(
					"'{}': this slice supports one scalar string positional",
					binding.field
				)));
			}
		}
	}

	for field in fields.keys() {
		if !bound.contains(field.as_str()) {
			return Err(RegistrationError(format!(
				"missing CLI binding for '{field}'"
			)));
		}
	}

	match (&shape.group, &bindings.group) {
		(Some(group), Some((field, anchor)))
			if &group.field == field
				&& matches!(
					group.fields.get(anchor),
					Some(Scalar::String | Scalar::NativePath)
				) =>
		{
			Ok(())
		}
		(None, None) => Ok(()),
		_ => Err(RegistrationError(
			"repeated object group requires a matching string anchor binding".into(),
		)),
	}
}

pub(super) fn validate_aliases(
	action: &str,
	aliases: &[Alias],
	shape: &Shape,
) -> Result<(), RegistrationError> {
	if action == "help" {
		return Err(RegistrationError(
			"action name 'help' is reserved by the CLI".into(),
		));
	}

	let mut names = BTreeSet::from([action, "help"]);
	for alias in aliases {
		validate_name(&alias.name)?;
		if !names.insert(&alias.name) {
			return Err(RegistrationError(format!(
				"duplicate or reserved action name '{}'",
				alias.name
			)));
		}

		let mut assigned = BTreeSet::new();
		for (field, value) in &alias.assignments {
			if !assigned.insert(field) {
				return Err(RegistrationError(format!(
					"duplicate assignment for '{field}' in alias '{}'",
					alias.name
				)));
			}

			let valid = match shape.scalars.get(field) {
				Some(Scalar::String) => value.is_string(),
				Some(Scalar::Boolean) => value.is_boolean(),
				Some(Scalar::NativePath) | None => false,
			};
			if !valid {
				return Err(RegistrationError(format!(
					"alias '{}' has invalid scalar assignment for '{field}'",
					alias.name
				)));
			}
		}
	}

	Ok(())
}
