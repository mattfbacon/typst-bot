use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
	Render { code: String },
	Ast { code: String },
	Version,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Rendered {
	pub image: Vec<u8>,
	pub more_pages: Option<NonZeroUsize>,
}

pub type RenderResponse = Result<Rendered, String>;

pub type AstResponse = String;

#[derive(Debug, Serialize, Deserialize)]
pub struct VersionResponse {
	pub version: String,
	pub git_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
	Render(RenderResponse),
	Ast(AstResponse),
	Version(VersionResponse),
}
