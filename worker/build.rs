use std::path::Path;

fn main() {
	let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").unwrap();
	let manifest_path = Path::new(&manifest_dir).join("Cargo.toml");
	let metadata = cargo_metadata::MetadataCommand::new()
		.manifest_path(manifest_path)
		.exec()
		.unwrap();
	let typst = &metadata
		.packages
		.iter()
		.find(|package| package.name == "typst")
		.unwrap();
	let version = &typst.version;

	println!("cargo:rustc-env=TYPST_VERSION={version}");
}
