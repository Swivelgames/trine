//! Structured encoding, host compatibility and resolution of native paths.

use std::path::Path;
use trine::NativePath;

#[test]
fn unicode_paths_use_strings_and_resolve_against_context() {
	let path = NativePath::new("relative/api").unwrap();
	assert_eq!(serde_json::to_value(&path).unwrap(), "relative/api");
	assert_eq!(
		path.resolve(Path::new("workspace")),
		Path::new("workspace/relative/api")
	);
	assert_eq!(
		serde_json::from_str::<NativePath>("\"relative/api\"").unwrap(),
		path
	);
}

#[test]
fn malformed_paths_fail() {
	for value in [
		serde_json::json!([65, 55296]),
		serde_json::json!([97, 255]),
		serde_json::json!(null),
		serde_json::json!("bad\u{0}path"),
		serde_json::json!({"platform":"unix","bytes":"!bad"}),
		serde_json::json!({"platform":"windows","wide":[65536]}),
		serde_json::json!({"platform":"unix","bytes":"YQ==","extra":1}),
		serde_json::json!({"platform":"unknown","bytes":"YQ=="}),
	] {
		assert!(serde_json::from_value::<NativePath>(value).is_err());
	}
}

#[cfg(unix)]
#[test]
fn raw_bytes_round_trip_and_foreign_encoding_is_rejected() {
	use std::{ffi::OsString, os::unix::ffi::OsStringExt};
	let path = NativePath::new(OsString::from_vec(vec![b'a', 255])).unwrap();
	let encoded = serde_json::to_value(&path).unwrap();
	assert_eq!(
		encoded,
		serde_json::json!({"platform":"unix", "bytes":"Yf8="})
	);
	assert_eq!(serde_json::from_value::<NativePath>(encoded).unwrap(), path);
	assert!(
		serde_json::from_value::<NativePath>(serde_json::json!({"platform":"windows","wide":[65]}))
			.is_err()
	);
	assert!(
		serde_json::from_value::<NativePath>(serde_json::json!({"platform":"unix","bytes":"AA=="}))
			.is_err()
	);
}

#[cfg(windows)]
#[test]
fn unpaired_surrogate_round_trips_and_foreign_encoding_is_rejected() {
	use std::{ffi::OsString, os::windows::ffi::OsStringExt};
	let path = NativePath::new(OsString::from_wide(&[65, 0xd800])).unwrap();
	let encoded = serde_json::to_value(&path).unwrap();
	assert_eq!(
		encoded,
		serde_json::json!({"platform":"windows","wide":[65,55296]})
	);
	assert_eq!(serde_json::from_value::<NativePath>(encoded).unwrap(), path);
	assert!(
		serde_json::from_value::<NativePath>(serde_json::json!({"platform":"unix","bytes":"YQ=="}))
			.is_err()
	);
	assert!(
		serde_json::from_value::<NativePath>(serde_json::json!({"platform":"windows","wide":[0]}))
			.is_err()
	);
}

#[test]
fn invalid_codec_diagnostics_identify_the_problem() {
	let malformed =
		serde_json::from_value::<NativePath>(serde_json::json!({"platform":"unix","bytes":"!bad"}))
			.unwrap_err();
	assert!(malformed.to_string().contains("base64"), "{malformed}");
	let unknown = serde_json::from_value::<NativePath>(
		serde_json::json!({"platform":"unix","bytes":"YQ==","extra":1}),
	)
	.unwrap_err();
	assert!(unknown.to_string().contains("extra"), "{unknown}");
}
