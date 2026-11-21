//! Shell transport and registration, separate from candidate policy.
mod zsh;

use super::{Context, Shell};
use clap::Command;
use std::{ffi::OsString, io, path::Path};

pub(super) const VARIABLE: &str = "TRINE_COMPLETE";

pub(super) fn registration(program: &str, shell: Shell) -> Vec<u8> {
	// Templates are owned registration code, not modifications to generated output.
	// Program names are validated by Application before reaching this boundary.
	let template = match shell {
		Shell::Bash => include_str!("shell/bash.bash"),
		Shell::Zsh => include_str!("shell/zsh.zsh"),
		Shell::Fish => include_str!("shell/fish.fish"),
		Shell::PowerShell => include_str!("shell/powershell.ps1"),
		_ => return Vec::new(),
	};

	template
		.replace("@PROGRAM@", program)
		.replace("@IDENTIFIER@", &program.replace('-', "_"))
		.replace(
			"@WINDOWS_PATHS@",
			if cfg!(windows) { "true" } else { "false" },
		)
		.into_bytes()
}

pub(super) fn respond(
	arguments: Vec<OsString>,
	cwd: &Path,
	select_command: impl FnOnce(&[OsString], usize) -> Command,
) -> io::Result<Vec<u8>> {
	let shell: Shell = std::env::var(VARIABLE)
		.ok()
		.and_then(|value| value.parse().ok())
		.ok_or_else(|| io::Error::other("invalid completion shell"))?;
	let mut context = request(shell, arguments, cwd)?;
	context.slash_paths = cfg!(windows) && matches!(shell, Shell::Bash | Shell::Zsh | Shell::Fish);

	// Readline replaces only the word after '=' or ':'; the engine completes the
	// whole logical value. Remove exactly the prefix Readline will retain.
	let bash_prefix = (shell == Shell::Bash)
		.then(|| {
			let word = std::env::var("TRINE_COMPLETE_WORD").ok()?;
			context
				.arguments
				.get(context.index)?
				.to_str()?
				.strip_suffix(&word)
				.map(str::to_owned)
		})
		.flatten();

	let metadata = std::env::var_os("TRINE_COMPLETE_METADATA").is_some();
	let grouped = shell == Shell::Zsh && metadata;
	let command = select_command(&context.arguments, context.index);
	let candidates = super::candidates(command, context, grouped)?;
	if grouped {
		return Ok(zsh::encode(&candidates));
	}

	let separator = if shell == Shell::Bash { "\u{b}" } else { "\n" };
	let values: Vec<_> = candidates
		.iter()
		.filter_map(|candidate| {
			// Shell line protocols cannot represent these paths faithfully.
			let value = candidate
				.get_value()
				.to_str()
				.filter(|value| !value.chars().any(char::is_control))?;

			if metadata
				&& matches!(shell, Shell::Fish | Shell::PowerShell)
				&& let Some(description) = candidate.get_help()
			{
				let description = description.to_string();
				if !description.is_empty() && !description.chars().any(char::is_control) {
					return Some(format!("{value}\t{description}"));
				}
			}

			Some(
				value
					.strip_prefix(bash_prefix.as_deref().unwrap_or(""))
					.unwrap_or(value)
					.to_owned(),
			)
		})
		.collect();
	let body = values.join(separator);

	if shell == Shell::Bash && std::env::var_os("TRINE_COMPLETE_QUOTE").is_some() {
		let quote = std::env::var_os("TRINE_COMPLETE_LINE")
			.and_then(|line| {
				let point = std::env::var("TRINE_COMPLETE_POINT").ok()?.parse().ok()?;
				super::bash_input::quote(&line, point)
			})
			.unwrap_or("unquoted");

		return Ok(format!("{quote}{separator}{body}").into_bytes());
	}

	Ok(body.into_bytes())
}

fn request(shell: Shell, arguments: Vec<OsString>, cwd: &Path) -> io::Result<Context> {
	if let (Shell::Bash, Some(line)) = (shell, std::env::var_os("TRINE_COMPLETE_LINE")) {
		let point = std::env::var("TRINE_COMPLETE_POINT")
			.ok()
			.and_then(|point| point.parse().ok())
			.ok_or_else(|| io::Error::other("invalid Bash cursor"))?;

		return super::bash_input::context(&line, point, cwd)
			.ok_or_else(|| io::Error::other("nonliteral Bash input"));
	}

	let separator = arguments
		.iter()
		.position(|value| value == "--")
		.ok_or_else(|| io::Error::other("missing completion arguments"))?;
	let mut arguments: Vec<_> = arguments.into_iter().skip(separator + 1).collect();
	let index = std::env::var("_CLAP_COMPLETE_INDEX")
		.ok()
		.and_then(|value| value.parse().ok())
		.filter(|index| *index <= arguments.len())
		.ok_or_else(|| io::Error::other("invalid completion cursor"))?;

	if index == arguments.len() {
		arguments.push(OsString::new());
	}

	Ok(Context::new(arguments, index, cwd.to_owned()))
}
