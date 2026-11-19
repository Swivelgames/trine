use schemars::JsonSchema;
use serde::Deserialize;
use std::ffi::OsString;
use trine::{
	Action, NativePath,
	cli::{Alias, Bindings, Cli, Parsed},
};

#[derive(Debug, Deserialize, JsonSchema)]
struct Selection {
	task: String,
	#[serde(default)]
	projects: Vec<NativePath>,
	#[serde(default)]
	repos: Vec<Repo>,
	#[serde(default)]
	checkout: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct Repo {
	path: NativePath,
	name: Option<String>,
}

fn bindings() -> Bindings {
	Bindings::new()
		.positional("task")
		.positional("projects")
		.native_path("projects")
		.group("repos", "path", "repo")
		.native_path("repos[].path")
		.option("repos[].name", "repo-name")
		.option("checkout", "checkout")
}

fn cli() -> Cli<Selection, (), (), ()> {
	let action = Action::new("create", "Create", |_, _| async { Ok(()) }).unwrap();
	let alias = Alias::new("checkout").set("checkout", true);
	Cli::new("demo", "1", action, bindings(), vec![alias]).unwrap()
}

#[test]
fn a_trailing_list_preserves_interleaved_options_and_native_values() {
	#[cfg(unix)]
	let native = {
		use std::os::unix::ffi::OsStringExt;
		OsString::from_vec(b"api-\xff".to_vec())
	};
	#[cfg(windows)]
	let native = {
		use std::os::windows::ffi::OsStringExt;
		OsString::from_wide(&[65, 0xd800])
	};

	let mut args: Vec<OsString> = [
		"demo",
		"checkout",
		"feature",
		"web",
		"--repo=one",
		"--repo-name=first",
	]
	.map(Into::into)
	.into();
	args.push(native.clone());
	args.extend(
		[
			"--repo",
			"two",
			"third",
			"--repo-name=second",
			"--",
			"-option",
		]
		.map(OsString::from),
	);
	let Parsed::Input(input) = cli().parse(args).unwrap() else {
		panic!("expected input")
	};

	assert_eq!(input.task, "feature");
	assert!(input.checkout);
	assert_eq!(
		input
			.projects
			.iter()
			.map(|path| path.as_path().as_os_str())
			.collect::<Vec<_>>(),
		[
			OsString::from("web"),
			native,
			"third".into(),
			"-option".into()
		]
	);
	assert_eq!(input.repos.len(), 2);
	assert_eq!(input.repos[0].path, NativePath::new("one").unwrap());
	assert_eq!(input.repos[0].name.as_deref(), Some("first"));
	assert_eq!(input.repos[1].path, NativePath::new("two").unwrap());
	assert_eq!(input.repos[1].name.as_deref(), Some("second"));
}

#[test]
fn an_omitted_list_uses_the_inputs_default() {
	let Parsed::Input(input) = cli()
		.parse(["demo", "create", "--repo=api", "feature"])
		.unwrap()
	else {
		panic!("expected input");
	};
	assert!(input.projects.is_empty());
	assert_eq!(input.repos.len(), 1);
}

#[test]
fn a_standalone_required_list_accepts_one_or_more_native_paths() {
	#[derive(Debug, Deserialize, JsonSchema)]
	struct Track {
		roots: Vec<NativePath>,
	}
	let action: Action<Track, (), (), ()> =
		Action::new("track", "Track roots", |_, _| async { Ok(()) }).unwrap();
	let bindings = Bindings::new().positional("roots").native_path("roots");
	let cli = Cli::new("demo", "1", action, bindings, vec![]).unwrap();

	assert!(cli.parse(["demo", "track"]).is_err());
	assert!(cli.parse(["demo", "track", "a\0b"]).is_err());
	let Parsed::Input(input) = cli.parse(["demo", "track", "a", "b"]).unwrap() else {
		panic!("expected input")
	};
	assert_eq!(
		input.roots,
		[NativePath::new("a").unwrap(), NativePath::new("b").unwrap()]
	);
}

#[test]
fn a_required_positional_cannot_follow_an_optional_prefix_even_on_an_alias() {
	#[derive(Deserialize, JsonSchema)]
	#[allow(dead_code)]
	struct Track {
		source: String,
		roots: Vec<NativePath>,
	}
	let action: Action<Track, (), (), ()> =
		Action::new("track", "Track roots", |_, _| async { Ok(()) }).unwrap();
	let bindings = Bindings::new()
		.positional("source")
		.positional("roots")
		.native_path("roots");
	let alias = Alias::new("all").set("source", "here");
	assert!(Cli::new("demo", "1", action, bindings, vec![alias]).is_err());
}
