fn main() {
	let sha = std::process::Command::new("git")
		.args(["rev-parse", "HEAD"])
		.output()
		.unwrap()
		.stdout;
	let sha = String::from_utf8(sha).unwrap();
	let sha = sha.trim();
	println!("cargo:rustc-env=BUILD_SHA={sha}");
}
