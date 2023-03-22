use std::io::Read as _;
use std::sync::Arc;

use comemo::Prehashed;
use typst::diag::{FileError, FileResult};
use typst::eval::Library;
use typst::font::{Font, FontBook};
use typst::syntax::{Source, SourceId};
use typst::util::Buffer;

use crate::FILE_NAME;

pub struct Sandbox {
	library: Prehashed<Library>,
	book: Prehashed<FontBook>,
	fonts: Vec<Font>,
}

fn fonts() -> Vec<Font> {
	[
		include_bytes!("../fonts/LinLibertine_R.ttf.deflate") as &[u8],
		include_bytes!("../fonts/LinLibertine_RB.ttf.deflate"),
		include_bytes!("../fonts/LinLibertine_RBI.ttf.deflate"),
		include_bytes!("../fonts/LinLibertine_RI.ttf.deflate"),
		include_bytes!("../fonts/NewCMMath-Book.otf.deflate"),
		include_bytes!("../fonts/NewCMMath-Regular.otf.deflate"),
		include_bytes!("../fonts/DejaVuSansMono.ttf.deflate"),
		include_bytes!("../fonts/DejaVuSansMono-Bold.ttf.deflate"),
		include_bytes!("../fonts/TwitterColorEmoji.ttf.deflate"),
	]
	.into_iter()
	.flat_map(|compressed| {
		let mut bytes = Vec::new();
		flate2::bufread::ZlibDecoder::new(compressed)
			.read_to_end(&mut bytes)
			.unwrap();
		let buffer = Buffer::from(bytes);
		Font::iter(buffer)
	})
	.collect()
}

fn make_source(source: String) -> Source {
	Source::new(SourceId::from_u16(0), FILE_NAME.as_ref(), source)
}

pub struct WithSource {
	sandbox: Arc<Sandbox>,
	source: Source,
}

impl Sandbox {
	pub fn new() -> Self {
		let fonts = fonts();

		Self {
			library: Prehashed::new(typst_library::build()),
			book: Prehashed::new(FontBook::from_fonts(&fonts)),
			fonts,
		}
	}

	pub fn with_source(self: Arc<Self>, source: String) -> WithSource {
		WithSource {
			sandbox: self,
			source: make_source(source),
		}
	}
}

impl WithSource {
	pub fn into_source(self) -> Source {
		self.source
	}
}

impl typst::World for WithSource {
	fn library(&self) -> &Prehashed<Library> {
		&self.sandbox.library
	}

	fn main(&self) -> &Source {
		&self.source
	}

	fn resolve(&self, path: &std::path::Path) -> FileResult<SourceId> {
		Err(FileError::NotFound(path.into()))
	}

	fn source(&self, id: SourceId) -> &Source {
		assert_eq!(id, self.source.id());
		&self.source
	}

	fn book(&self) -> &Prehashed<FontBook> {
		&self.sandbox.book
	}

	fn font(&self, id: usize) -> Option<Font> {
		self.sandbox.fonts.get(id).cloned()
	}

	fn file(&self, path: &std::path::Path) -> FileResult<Buffer> {
		Err(FileError::NotFound(path.into()))
	}
}
