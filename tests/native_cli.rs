//! Lossless CLI parsing of native paths in grouped and scalar inputs.

use schemars::JsonSchema;
use serde::Deserialize;
use std::ffi::OsString;
use trine::{
	Action, Cancellation, InvocationContext, NativePath,
	cli::{Bindings, Cli, Parsed},
};

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
struct Input {
	repositories: Vec<Repo>,
	remote: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
struct Repo {
	path: NativePath,
	remote: Option<String>,
}

fn cli() -> Cli<Input, Input, (), ()> {
	let action = Action::new(
		"run",
		"Native inputs",
		|input: Input, _: InvocationContext<()>| async move { Ok(input) },
	)
	.unwrap();
	Cli::new(
		"demo",
		"1",
		action,
		Bindings::new()
			.group("repositories", "path", "repo")
			.native_path("repositories[].path")
			.option("repositories[].remote", "repo-remote")
			.option("remote", "remote"),
		vec![],
	)
	.unwrap()
}

#[test]
fn grouped_paths_retain_order_and_common_options() {
	let cli = cli();
	let Parsed::Input(input) = cli
		.parse([
			"demo",
			"run",
			"--repo=api",
			"--repo-remote=upstream",
			"--remote=origin",
			"--repo=web",
		])
		.unwrap()
	else {
		panic!()
	};
	assert_eq!(input.repositories[0].path, NativePath::new("api").unwrap());
	assert_eq!(input.repositories[1].path, NativePath::new("web").unwrap());
	assert_eq!(input.repositories[0].remote.as_deref(), Some("upstream"));
	assert_eq!(input.remote.as_deref(), Some("origin"));
}

#[cfg(unix)]
fn native_argument() -> OsString {
	use std::os::unix::ffi::OsStringExt;
	OsString::from_vec(vec![b'a', 255])
}

#[cfg(windows)]
fn native_argument() -> OsString {
	use std::os::windows::ffi::OsStringExt;
	OsString::from_wide(&[65, 0xd800])
}

#[test]
fn native_cli_direct_and_structured_input_preserve_exact_path() {
	let path = native_argument();
	let Parsed::Input(input) = cli()
		.parse([
			OsString::from("demo"),
			"run".into(),
			"--repo".into(),
			path.clone(),
		])
		.unwrap()
	else {
		panic!()
	};
	let native = NativePath::new(path).unwrap();
	let encoded = serde_json::json!({"repositories":[{"path":native}]});
	assert_eq!(input, serde_json::from_value::<Input>(encoded).unwrap());
	let direct: Action<Input, Input, (), ()> =
		Action::new("run", "Direct", |input, _| async { Ok(input) }).unwrap();
	let result =
		smol::block_on(direct.invoke(input, InvocationContext::unobserved(Cancellation::new())))
			.unwrap();
	assert_eq!(result.repositories[0].path, native);
}

#[test]
fn native_nul_is_rejected_before_handler() {
	assert!(cli().parse(["demo", "run", "--repo", "bad\0path"]).is_err());
}

#[test]
fn optional_renamed_native_fields_use_serde_defaults_and_reject_invalid_input_before_execution() {
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};

	#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
	#[serde(deny_unknown_fields)]
	struct Paths {
		#[serde(rename = "state-dir")]
		state_dir: NativePath,
		other: Option<NativePath>,
		#[serde(default)]
		verbose: bool,
	}

	let calls = Arc::new(AtomicUsize::new(0));
	let handler_calls = calls.clone();
	let action = Action::new(
		"run",
		"Read paths",
		move |input: Paths, _: InvocationContext<()>| {
			handler_calls.fetch_add(1, Ordering::SeqCst);
			async move { Ok::<_, ()>(input) }
		},
	)
	.unwrap();
	let cli = Cli::new(
		"demo",
		"1",
		action,
		Bindings::new()
			.option("state-dir", "state-dir")
			.native_path("state-dir")
			.option("other", "other")
			.native_path("other")
			.option("verbose", "verbose"),
		vec![],
	)
	.unwrap();

	let Parsed::Input(omitted) = cli.parse(["demo", "run", "--state-dir", "state"]).unwrap() else {
		panic!();
	};
	assert_eq!(
		omitted,
		serde_json::from_value(serde_json::json!({"state-dir": "state"})).unwrap()
	);

	let native = native_argument();
	let Parsed::Input(present) = cli
		.parse([
			OsString::from("demo"),
			"run".into(),
			"--state-dir".into(),
			"state".into(),
			"--other".into(),
			native.clone(),
		])
		.unwrap()
	else {
		panic!()
	};
	assert_eq!(present.other, Some(NativePath::new(native).unwrap()));

	let error = cli.parse(["demo", "run"]).unwrap_err();
	assert!(error.0.contains("state-dir"), "{error}");
	assert!(
		cli.parse(["demo", "run", "--state-dir", "bad\0path"])
			.is_err()
	);
	assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn flattened_native_input_reports_custom_assembly_remedy_before_handler() {
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};

	#[derive(Debug, Deserialize, JsonSchema)]
	struct Paths {
		path: NativePath,
	}

	#[derive(Debug, Deserialize, JsonSchema)]
	struct Flattened {
		#[serde(flatten)]
		paths: Paths,
	}

	let calls = Arc::new(AtomicUsize::new(0));
	let handler_calls = calls.clone();
	let action = Action::new(
		"run",
		"Read flattened path",
		move |input: Flattened, _: InvocationContext<()>| {
			handler_calls.fetch_add(1, Ordering::SeqCst);
			async move { Ok::<_, ()>(input.paths.path) }
		},
	)
	.unwrap();
	let cli = Cli::new(
		"demo",
		"1",
		action,
		Bindings::new().option("path", "path").native_path("path"),
		vec![],
	)
	.unwrap();

	let error = cli
		.parse([
			OsString::from("demo"),
			"run".into(),
			"--path".into(),
			native_argument(),
		])
		.unwrap_err();
	for detail in [
		"field `path`",
		"#[serde(flatten)]",
		"Cli::with_input",
		"InputFields::take_path",
	] {
		assert!(error.0.contains(detail), "{error}");
	}
	assert_eq!(calls.load(Ordering::SeqCst), 0);
}
