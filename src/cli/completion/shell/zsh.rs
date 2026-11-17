//! Private line framing keeps display metadata separate from insertion values.
use clap_complete::engine::CompletionCandidate;
use std::fmt::Write;

pub(super) fn encode(candidates: &[CompletionCandidate]) -> Vec<u8> {
	let mut output = String::new();

	for candidate in candidates {
		let Some(value) = candidate
			.get_value()
			.to_str()
			.filter(|value| representable(value))
		else {
			continue;
		};

		let group = candidate
			.get_tag()
			.map(ToString::to_string)
			.filter(|group| !group.is_empty() && representable(group))
			.unwrap_or_else(|| "Values".into());
		let help = candidate
			.get_help()
			.map(ToString::to_string)
			.filter(|help| representable(help))
			.unwrap_or_default();

		// Empty descriptions are valid: the shell reads exactly three lines per row.
		writeln!(output, "{group}\n{value}\n{help}").expect("writing to a String cannot fail");
	}

	output.into_bytes()
}

fn representable(value: &str) -> bool {
	!value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn display_metadata_cannot_change_framing_or_insertion() {
		let candidates = [
			CompletionCandidate::new("fix/name:literal%")
				.tag(Some("Tasks:%B".into()))
				.help(Some("literal: 100%".into())),
			CompletionCandidate::new("safe")
				.tag(Some("bad\nheading".into()))
				.help(Some("bad\u{1b}text".into())),
			CompletionCandidate::new("bad\nvalue"),
		];

		assert_eq!(
			String::from_utf8(encode(&candidates)).unwrap(),
			"Tasks:%B\nfix/name:literal%\nliteral: 100%\nValues\nsafe\n\n"
		);
	}
}
