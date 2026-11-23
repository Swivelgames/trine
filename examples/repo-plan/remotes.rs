//! Resolve the repository at the cursor and suggest its configured Git remotes.

use std::{
	ffi::{OsStr, OsString},
	process::Command,
};
use trine::cli::completion::{Context, OptionValue};

pub(super) fn suggest(context: &Context, prefix: &OsStr) -> Vec<OsString> {
	let Some(row) = context.current_group("repos") else {
		return Vec::new();
	};
	let OptionValue::Present(path) = row.field("path") else {
		return Vec::new();
	};
	let Some(prefix) = prefix.to_str() else {
		return Vec::new();
	};

	let output = Command::new("git")
		.arg("-C")
		.arg(context.cwd.join(path))
		.arg("remote")
		.output();
	let Ok(output) = output else {
		return Vec::new();
	};
	if !output.status.success() {
		return Vec::new();
	}

	output
		.stdout
		.split(|byte| *byte == b'\n')
		.filter_map(|line| std::str::from_utf8(line).ok())
		.map(|line| line.trim_end_matches('\r'))
		.filter(|name| !name.is_empty() && name.starts_with(prefix))
		.map(OsString::from)
		.collect()
}
