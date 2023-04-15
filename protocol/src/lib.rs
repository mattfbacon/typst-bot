use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};

pub type Request = String;

#[derive(Debug, Serialize, Deserialize)]
pub struct Output {
	pub image: Vec<u8>,
	pub more_pages: Option<NonZeroUsize>,
}

pub type Response = Result<Output, String>;
