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
	rust_2018_idioms,
	unused_qualifications
)]
#![warn(clippy::pedantic)]
#![forbid(unsafe_code)]

mod bot;
mod worker;

const SOURCE_URL: &str = "https://github.com/mattfbacon/typst-bot";

#[tokio::main(flavor = "current_thread")]
async fn main() {
	self::bot::run().await;
}
