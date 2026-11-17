//! Recognize presentation options without interpreting native action values.
use std::ffi::OsString;

/// Used to render parse failures before a complete Clap match is available.
/// Values after `--`, and values embedded in `--option=value`, are never options.
pub fn json_requested(arguments: &[OsString]) -> bool {
	arguments
		.iter()
		.skip(1)
		.take_while(|argument| *argument != "--")
		.any(|argument| argument == "--json")
}

pub(crate) fn without_json(arguments: Vec<OsString>) -> Vec<OsString> {
	let mut positional = false;
	arguments
		.into_iter()
		.enumerate()
		.filter_map(|(index, argument)| {
			if index == 0 {
				return Some(argument);
			}
			if argument == "--" {
				positional = true;
			}
			if !positional && argument == "--json" {
				return None;
			}
			Some(argument)
		})
		.collect()
}
