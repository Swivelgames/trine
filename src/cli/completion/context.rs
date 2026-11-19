//! Native, incomplete input supplied to field-specific completion providers.
use std::{ffi::OsString, path::PathBuf};

/// All words remain available, including placement flags after the cursor.
#[derive(Clone, Debug)]
pub struct Context {
	pub arguments: Vec<OsString>,
	pub index: usize,
	pub cwd: PathBuf,
	pub(super) input: Option<super::input::Input>,
}

/// A placement option may be absent, complete, or unsafe to infer from partial input.
pub enum OptionValue<'a> {
	Absent,
	Incomplete,
	Present(&'a std::ffi::OsStr),
	Ambiguous,
}

impl Context {
	pub fn new(arguments: Vec<OsString>, index: usize, cwd: PathBuf) -> Self {
		Self {
			arguments,
			index,
			cwd,
			input: None,
		}
	}

	/// A scalar field, including alias assignments and complete arguments after the cursor.
	pub fn field(&self, name: &str) -> OptionValue<'_> {
		self.input
			.as_ref()
			.map_or(OptionValue::Absent, |input| input.field(name))
	}

	/// Completed occurrences of a list field in argument order, excluding the word
	/// being edited. A known empty list and unavailable context are distinct.
	pub fn values(&self, name: &str) -> Option<&[OsString]> {
		let input = self.input.as_ref().filter(|input| input.valid)?;
		input.lists.get(name).map(Vec::as_slice)
	}

	/// All explicitly supplied rows; unavailable role context is distinct from no rows.
	pub fn groups(&self, name: &str) -> Option<&[super::Group]> {
		let input = self.input.as_ref().filter(|input| input.valid)?;
		input.groups.get(name).map(Vec::as_slice)
	}

	/// The row whose anchor precedes the cursor, never a later repository's row.
	pub fn current_group(&self, name: &str) -> Option<&super::Group> {
		self.groups(name)?
			.iter()
			.rev()
			.find(|row| row.anchor_index < self.index)
	}

	/// Inspect a bound long option during a provider callback, using parsed field roles.
	pub fn option(&self, long: &str) -> OptionValue<'_> {
		let Some(input) = &self.input else {
			return OptionValue::Absent;
		};

		input
			.options
			.get(long)
			.map_or(OptionValue::Absent, |field| input.field(field))
	}
}
