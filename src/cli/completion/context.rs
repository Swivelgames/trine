//! Native, incomplete input supplied to field-specific completion providers.
use std::{ffi::OsString, path::PathBuf};

/// All words remain available, including placement flags after the cursor.
#[derive(Clone, Debug)]
pub struct Context {
	pub arguments: Vec<OsString>,
	pub index: usize,
	pub cwd: PathBuf,
}

/// A placement option may be absent, complete, or unsafe to infer from partial input.
pub enum OptionValue<'a> {
	Absent,
	Present(&'a std::ffi::OsStr),
	Ambiguous,
}

impl Context {
	/// Inspect a named option without decoding native values or validating action input.
	/// Duplicate options, missing values and values under the cursor are ambiguous.
	pub fn option(&self, long: &str) -> OptionValue<'_> {
		use clap_lex::OsStrExt;

		let flag = format!("--{long}");
		let prefix = format!("{flag}=");

		let mut found = None;
		let mut arguments = self.arguments.iter().enumerate().skip(1);

		while let Some((index, argument)) = arguments.next() {
			if argument == "--" {
				break;
			}

			let value = if argument == flag.as_str() {
				let Some((value_index, value)) = arguments.next() else {
					return OptionValue::Ambiguous;
				};

				if value_index == self.index || value.starts_with("-") {
					return OptionValue::Ambiguous;
				}

				Some(value.as_os_str())
			} else {
				argument.strip_prefix(&prefix)
			};

			if let Some(value) = value {
				if found.is_some() || index == self.index || value.is_empty() {
					return OptionValue::Ambiguous;
				}

				found = Some(value);
			}
		}

		found.map_or(OptionValue::Absent, OptionValue::Present)
	}
}
