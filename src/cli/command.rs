//! Build Clap commands and attach field metadata for parsing and completion.

use super::{Alias, Bindings, bindings::Binding, schema::Scalar};
use clap::{Arg, ArgAction, Command};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) const ALTERNATIVE_POSITIONAL: &str = "alternative-positional";

pub(super) fn alias_command(command: &Command, alias: &Alias) -> Command {
	let mut command = command.clone().name(alias.name.clone());
	if let Some(description) = &alias.description {
		command = command.about(description.clone());
	}

	command.mut_args(|argument| {
		let Some(mut field) = argument.get::<super::completion::Field>().cloned() else {
			return argument;
		};
		let Some((_, value)) = alias
			.assignments
			.iter()
			.find(|(name, _)| name == &field.name)
		else {
			return argument;
		};

		field.assigned = Some(
			value
				.as_str()
				.map(str::to_owned)
				.unwrap_or_else(|| value.to_string())
				.into(),
		);
		let named_option = argument.get_long().is_some();
		argument.add(field).required(false).hide(named_option)
	})
}

pub(super) fn build_command(
	name: &str,
	description: &str,
	bindings: &Bindings,
	fields: &BTreeMap<String, Scalar>,
	schema: &Value,
) -> Command {
	let mut command = Command::new(name.to_owned()).about(description.to_owned());
	for binding in &bindings.fields {
		let argument = field_argument(binding, fields[&binding.field], bindings, schema);
		if let Some((field, name)) = &bindings.positional_alternative
			&& field == &binding.field
		{
			let positional = argument
				.clone()
				.id(ALTERNATIVE_POSITIONAL)
				.long(None::<&str>)
				.value_name(name.clone())
				.action(ArgAction::Set)
				.conflicts_with(argument.get_id().clone());
			command = command.arg(positional);
		}

		command = command.arg(argument);
	}

	if let Some(examples) = &bindings.examples {
		command = command.after_help(examples.clone());
	}
	command
}

fn field_argument(binding: &Binding, scalar: Scalar, bindings: &Bindings, schema: &Value) -> Arg {
	// Append retains every occurrence. Normalization collects string arrays
	// and grouped rows, while rejecting duplicate scalar fields.
	let value_name = binding
		.field
		.rsplit('.')
		.next()
		.unwrap()
		.to_ascii_uppercase();
	let mut argument = Arg::new(argument_id(&binding.field))
		.value_name(value_name)
		.action(ArgAction::Append);

	if let Some(help) = schema["properties"][&binding.field]["description"].as_str() {
		argument = argument.help(help.to_owned());
	}
	if let Some(long) = &binding.long {
		argument = argument.long(long.clone());
	}

	argument = match scalar {
		Scalar::String | Scalar::StringList => argument
			.num_args(1)
			.value_parser(clap::builder::StringValueParser::new()),
		Scalar::NativePath | Scalar::NativePathList => argument
			.value_hint(clap::ValueHint::AnyPath)
			.num_args(1)
			.value_parser(clap::builder::OsStringValueParser::new()),
		Scalar::Boolean => argument
			.hide_possible_values(true)
			.num_args(0..=1)
			.require_equals(true)
			.default_missing_value("true")
			.value_parser(clap::value_parser!(bool)),
	};

	if let Some(provider) = bindings.completions.get(&binding.field) {
		argument = argument.add(provider.clone());
	}
	let group = bindings.group.as_ref().and_then(|(group, anchor)| {
		let child = binding.field.strip_prefix(&format!("{group}[]."))?;
		Some(super::completion::GroupField {
			name: group.clone(),
			child: child.into(),
			anchor: child == anchor,
		})
	});
	argument = argument.add(super::completion::Field {
		name: binding.field.clone(),
		group,
		repeated: scalar.repeated(),
		assigned: None,
	});

	if binding.long.is_none() {
		let required = schema["required"]
			.as_array()
			.is_some_and(|names| names.iter().any(|name| name == &binding.field));
		argument = argument.required(required);
		if scalar.repeated() {
			argument = argument.num_args(1..);
		} else {
			argument = argument.action(ArgAction::Set);
		}
	}
	argument
}

pub(super) fn argument_id(field: &str) -> String {
	// Completion generators can expose IDs as shell syntax, so keep them free of
	// separators and quoting characters. Encode bytes rather than replacing them
	// to preserve distinct schema field names (including the escape marker).
	let mut id = String::from("input-");
	for byte in field.bytes() {
		if byte.is_ascii_alphanumeric() || byte == b'-' {
			id.push(char::from(byte));
		} else {
			use std::fmt::Write;
			write!(id, "_{byte:02x}").expect("writing to a String cannot fail");
		}
	}
	id
}

#[cfg(test)]
mod tests {
	use super::argument_id;
	use std::collections::BTreeSet;

	#[test]
	fn argument_ids_are_shell_safe_without_collapsing_schema_fields() {
		let fields = [
			"task",
			"a:b",
			"a_b",
			"a_3ab",
			"a-b",
			"repo[].path",
			"a'b",
			"路径",
		];
		let ids: BTreeSet<_> = fields.into_iter().map(argument_id).collect();

		assert_eq!(argument_id("task"), "input-task");
		assert_eq!(ids.len(), fields.len());
		assert!(ids.iter().all(|id| {
			id.bytes()
				.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
		}));
	}
}
