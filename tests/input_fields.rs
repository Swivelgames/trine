//! Typed input assembly from named fields, native paths and Serde defaults.

use serde::Deserialize;
use serde_json::json;
use trine::{NativePath, cli::InputFields};

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Input {
	#[serde(rename = "state-dir")]
	state_dir: NativePath,
	optional: Option<NativePath>,
	#[serde(default = "default_label")]
	label: String,
	#[serde(default)]
	verbose: bool,
	#[serde(default)]
	repositories: Vec<Repository>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Repository {
	path: NativePath,
	remote: Option<String>,
}

fn default_label() -> String {
	"workspace".into()
}

fn fields() -> InputFields {
	let mut fields = InputFields::default();
	fields
		.paths
		.insert("state-dir".into(), NativePath::new("state").unwrap());
	fields
}

#[test]
fn renamed_native_fields_and_omitted_defaults_match_serde() {
	let actual = fields().deserialize::<Input>().unwrap();
	let expected = serde_json::from_value(json!({"state-dir": "state"})).unwrap();
	assert_eq!(actual, expected);
}

#[test]
fn present_optional_native_path_and_scalars_deserialize_directly() {
	let mut fields = fields();
	fields
		.paths
		.insert("optional".into(), NativePath::new("other").unwrap());
	fields.scalars.insert("verbose".into(), json!(true));
	fields.scalars.insert("label".into(), json!("custom"));
	let actual = fields.deserialize::<Input>().unwrap();
	assert_eq!(actual.optional, Some(NativePath::new("other").unwrap()));
	assert!(actual.verbose);
	assert_eq!(actual.label, "custom");
}

#[test]
fn missing_and_mistyped_fields_have_useful_diagnostics() {
	let missing = InputFields::default().deserialize::<Input>().unwrap_err();
	assert!(missing.0.contains("missing field `state-dir`"), "{missing}");

	let mut fields = fields();
	fields.scalars.insert("verbose".into(), json!("yes"));
	let invalid = fields.deserialize::<Input>().unwrap_err();
	assert!(invalid.0.contains("field `verbose`"), "{invalid}");
	assert!(invalid.0.contains("boolean"), "{invalid}");
}

#[test]
fn group_errors_identify_row_and_field() {
	let mut fields = fields();
	let mut row = InputFields::default();
	row.scalars.insert("path".into(), json!(42));
	fields.groups.insert("repositories".into(), vec![row]);
	let error = fields.deserialize::<Input>().unwrap_err();
	for context in ["repositories", "row [0]", "path"] {
		assert!(error.0.contains(context), "{error}");
	}
}

#[test]
fn unknown_fields_follow_the_input_type_policy() {
	let mut strict = fields();
	strict
		.paths
		.insert("extra".into(), NativePath::new("extra").unwrap());
	let error = strict.deserialize::<Input>().unwrap_err();
	assert!(error.0.contains("unknown field `extra`"), "{error}");

	#[derive(Deserialize)]
	struct Permissive {
		#[serde(rename = "state-dir")]
		state_dir: NativePath,
	}
	let mut permissive = fields();
	permissive
		.paths
		.insert("extra".into(), NativePath::new("extra").unwrap());
	assert_eq!(
		permissive.deserialize::<Permissive>().unwrap().state_dir,
		NativePath::new("state").unwrap()
	);
}

#[test]
fn ambiguous_representations_are_rejected_instead_of_overwritten() {
	let mut scalar_path = fields();
	scalar_path
		.scalars
		.insert("state-dir".into(), json!("other"));
	let error = scalar_path.deserialize::<Input>().unwrap_err();
	assert!(
		error.0.contains("state-dir") && error.0.contains("multiple input representations"),
		"{error}"
	);

	let mut scalar_group = fields();
	scalar_group
		.scalars
		.insert("repositories".into(), json!([]));
	scalar_group.groups.insert("repositories".into(), vec![]);
	let error = scalar_group.deserialize::<Input>().unwrap_err();
	assert!(
		error.0.contains("multiple input representations"),
		"{error}"
	);

	let mut path_group = fields();
	path_group.groups.insert("state-dir".into(), vec![]);
	let error = path_group.deserialize::<Input>().unwrap_err();
	assert!(
		error.0.contains("multiple input representations"),
		"{error}"
	);
}

#[test]
fn custom_assembly_can_still_extract_paths_and_groups() {
	let mut fields = fields();
	let mut row = InputFields::default();
	row.paths
		.insert("path".into(), NativePath::new("api").unwrap());
	fields.groups.insert("repositories".into(), vec![row]);
	let path = fields.take_path("state-dir").unwrap();
	let rows = fields.take_group("repositories");
	assert_eq!(path, NativePath::new("state").unwrap());
	let row = rows.into_iter().next().unwrap();
	let repository = row.deserialize::<Repository>().unwrap();
	assert_eq!(repository.path, NativePath::new("api").unwrap());
	assert_eq!(
		fields.deserialize::<serde_json::Value>().unwrap(),
		json!({})
	);
}

#[test]
fn native_values_are_not_silently_converted_to_strings_or_json() {
	#[derive(Debug, Deserialize)]
	struct Text {
		#[serde(rename = "state-dir")]
		_state_dir: String,
	}
	let text_error = fields().deserialize::<Text>().unwrap_err();
	assert!(text_error.0.contains("expected NativePath"), "{text_error}");
	let json_error = fields().deserialize::<serde_json::Value>().unwrap_err();
	assert!(json_error.0.contains("expected NativePath"), "{json_error}");
}
