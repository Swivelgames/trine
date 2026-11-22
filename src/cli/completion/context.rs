//! Native, incomplete input supplied to field-specific completion providers.
use std::{ffi::OsString, path::PathBuf};

/// All words remain available, including placement flags after the cursor.
#[derive(Clone, Debug)]
pub struct Context {
	/// Decoded native words, starting with the executable.
	pub arguments: Vec<OsString>,
	/// Index of the word being completed in `arguments`.
	pub index: usize,
	/// Working directory used to resolve relative path suggestions.
	pub cwd: PathBuf,
	pub(super) slash_paths: bool,
	pub(super) input: Option<super::input::Input>,
}

/// A placement option may be absent, complete, or unsafe to infer from partial input.
pub enum OptionValue<'a> {
	/// No occurrence supplied a value for this field.
	Absent,
	/// The option or its value is still being entered.
	Incomplete,
	/// A complete native value is available.
	Present(&'a std::ffi::OsStr),
	/// Conflicting or malformed input prevents a reliable choice.
	Ambiguous,
}

impl Context {
	/// Construct a completion query from shell words, a word index and a working directory.
	pub fn new(arguments: Vec<OsString>, index: usize, cwd: PathBuf) -> Self {
		Self {
			arguments,
			index,
			cwd,
			input: None,
			slash_paths: false,
		}
	}

	/// Format a filesystem suggestion for insertion. Domain values need no formatting.
	pub fn filesystem_candidate(&self, value: OsString) -> OsString {
		if !self.slash_paths {
			return value;
		}
		let Some(text) = value.to_str() else {
			return value;
		};

		// Verbatim and device paths need their native namespace and separators.
		if text.starts_with(r"\\?\") || text.starts_with(r"\\.\") || !text.contains('\\') {
			return value;
		}
		text.replace('\\', "/").into()
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn slash_style_preserves_windows_namespaces() {
		let mut context = Context::new(Vec::new(), 0, PathBuf::new());
		context.slash_paths = true;
		for (native, expected) in [
			(r"C:\dir\child", "C:/dir/child"),
			(r"\\server\share\dir", "//server/share/dir"),
			(r"\\?\C:\dir", r"\\?\C:\dir"),
			(r"\\?\UNC\server\share", r"\\?\UNC\server\share"),
			(r"\\.\pipe\name", r"\\.\pipe\name"),
		] {
			assert_eq!(
				context.filesystem_candidate(native.into()),
				OsString::from(expected)
			);
		}
	}
}
