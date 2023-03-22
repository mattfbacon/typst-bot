#![deny(
	absolute_paths_not_starting_with_crate,
	keyword_idents,
	macro_use_extern_crate,
	meta_variable_misuse,
	missing_abi,
	missing_copy_implementations,
	non_ascii_idents,
	nonstandard_style,
	noop_method_call,
	pointer_structural_match,
	private_in_public,
	rust_2018_idioms,
	unused_qualifications
)]
#![warn(clippy::pedantic)]
#![forbid(unsafe_code)]

mod bot;
mod render;
mod sandbox;

const FILE_NAME: &str = "<user input>";
const SOURCE_URL: &str = "https://github.com/mattfbacon/typst-bot";
const PREAMBLE: &str = "// Preamble\n#set page(width: auto, height: auto)\n// End preamble\n\n";

#[tokio::main(flavor = "current_thread")]
async fn main() {
	self::bot::run().await;
}
