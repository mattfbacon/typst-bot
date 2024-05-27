use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
	Render { code: String },
	Ast { code: String },
	Version,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Rendered {
	pub images: Vec<Vec<u8>>,
	pub more_pages: usize,
	pub warnings: String,
}

pub type RenderResponse = Result<Rendered, String>;

pub type AstResponse = String;

#[derive(Debug, Serialize, Deserialize)]
pub struct VersionResponse {
	pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
	Render(RenderResponse),
	Ast(AstResponse),
	Version(VersionResponse),
	/// This can be sent at any time and is not considered a final response for a request,
	/// but can be shown to the user in the meantime as a progress update.
	Progress(String),
}
