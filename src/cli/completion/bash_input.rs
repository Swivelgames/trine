//! Read literal Bash words without evaluating shell text or relying on COMP_WORDBREAKS.
use super::Context;
use std::{
	ffi::{OsStr, OsString},
	path::Path,
};

struct Word {
	start: usize,
	end: usize,
	value: Vec<u8>,
	quote: Option<u8>,
}

pub(super) fn context(line: &OsStr, point: usize, cwd: &Path) -> Option<Context> {
	let bytes = line.as_encoded_bytes();
	if point > bytes.len() || bytes.get(point).is_some_and(|byte| byte & 0xc0 == 0x80) {
		return None;
	}

	let words = tokenize(bytes)?;
	let current = words
		.iter()
		.position(|word| word.start <= point && point <= word.end);
	let index = current.unwrap_or_else(|| {
		words
			.iter()
			.position(|word| point < word.start)
			.unwrap_or(words.len())
	});

	let prefix = if let Some(current) = current {
		tokenize(&bytes[words[current].start..point])?
			.pop()
			.map(|word| word.value)
			.unwrap_or_default()
	} else {
		Vec::new()
	};

	let mut arguments: Vec<_> = words.into_iter().map(|word| native(word.value)).collect();
	if current.is_some() {
		arguments[index] = native(prefix);
	} else {
		arguments.insert(index, OsString::new());
	}

	Some(Context::new(arguments, index, cwd.to_owned()))
}

/// Quote state is private shell transport metadata, not domain input.
pub(super) fn quote(line: &OsStr, point: usize) -> Option<&'static str> {
	let bytes = line.as_encoded_bytes();
	let prefix = bytes.get(..point)?;
	let words = tokenize(prefix)?;
	let quote = words
		.last()
		.filter(|word| word.end == point)
		.and_then(|word| word.quote);

	Some(match quote {
		Some(b'\'') => "single",
		Some(b'"') => "double",
		_ => "unquoted",
	})
}

fn native(bytes: Vec<u8>) -> OsString {
	// These bytes originate from OsStr and only ASCII shell syntax was removed.
	// No non-ASCII encoding units are split or synthesized by the tokenizer.
	unsafe { OsString::from_encoded_bytes_unchecked(bytes) }
}

fn tokenize(bytes: &[u8]) -> Option<Vec<Word>> {
	let mut words = Vec::new();
	let mut index = 0;

	while index < bytes.len() {
		if bytes[index].is_ascii_whitespace() {
			index += 1;
			continue;
		}

		let start = index;
		let mut value = Vec::new();
		let mut quote = None;

		while index < bytes.len() {
			let byte = bytes[index];
			if quote.is_none() && byte.is_ascii_whitespace() {
				break;
			}

			let is_quote = matches!(byte, b'\'' | b'"') && (quote.is_none() || quote == Some(byte));
			if is_quote {
				quote = if quote.is_some() { None } else { Some(byte) };
			} else if byte == b'\\' && quote != Some(b'\'') {
				index += 1;
				let Some(&escaped) = bytes.get(index) else {
					break;
				};

				if quote == Some(b'"') && !matches!(escaped, b'$' | b'`' | b'"' | b'\\' | b'\n') {
					value.push(b'\\');
				}
				if escaped != b'\n' {
					value.push(escaped);
				}
			} else {
				// Expansion and command operators are deliberately not interpreted.
				if quote != Some(b'\'') && matches!(byte, b'$' | b'`') {
					return None;
				}

				let expansion = matches!(byte, b'~' | b'*' | b'?' | b'[' | b']' | b'{' | b'}');
				let operator = matches!(byte, b';' | b'|' | b'&' | b'<' | b'>' | b'(' | b')');
				if quote.is_none() && (expansion || operator) {
					return None;
				}

				value.push(byte);
			}

			index += 1;
		}

		words.push(Word {
			start,
			end: index,
			value,
			quote,
		});
	}

	Some(words)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn equals_and_colons_are_values_not_readline_wordbreaks() {
		let line = "demo amend f --state-dir='/some:state path'";
		let request = context(OsStr::new(line), 12, Path::new("/")).unwrap();

		assert_eq!(request.index, 2);
		assert_eq!(
			request.arguments,
			["demo", "amend", "f", "--state-dir=/some:state path"].map(OsString::from)
		);
	}

	#[test]
	fn empty_current_and_unfinished_quotes_preserve_trailing_arguments() {
		let line = "demo amend  --state-dir=x";
		let request = context(OsStr::new(line), 11, Path::new("/")).unwrap();

		assert_eq!(
			request.arguments,
			["demo", "amend", "", "--state-dir=x"].map(OsString::from)
		);

		assert_eq!(
			context(
				OsStr::new("demo amend 'fix/o"),
				"demo amend 'fix/o".len(),
				Path::new("/")
			)
			.unwrap()
			.arguments[2],
			"fix/o"
		);
	}

	#[test]
	fn expansions_are_never_executed_or_guessed() {
		assert!(context(OsStr::new("demo $(touch surprise)"), 21, Path::new("/")).is_none());

		let line = "demo 'fix/$literal'";
		assert_eq!(
			context(OsStr::new(line), line.len(), Path::new("/"))
				.unwrap()
				.arguments[1],
			"fix/$literal"
		);
	}

	#[test]
	fn unresolved_path_expansions_are_not_literal_workspace_selection() {
		for value in [
			"~/state",
			"workspace*",
			"workspace?",
			"[ab]",
			"{one,two}",
			"{1..2}",
		] {
			let line = format!("demo amend f --state-dir={value}");

			assert!(
				context(OsStr::new(&line), 12, Path::new("/")).is_none(),
				"{line}"
			);
		}

		for value in [
			"'~/state'",
			"\"workspace*\"",
			"workspace\\*",
			"\\~",
			"\\{one,two\\}",
		] {
			let line = format!("demo amend f --state-dir={value}");

			assert!(
				context(OsStr::new(&line), 12, Path::new("/")).is_some(),
				"{line}"
			);
		}
	}
}
