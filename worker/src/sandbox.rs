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
	std::fs::read_dir("fonts")
		.unwrap()
		.map(Result::unwrap)
		.flat_map(|entry| {
			let bytes = std::fs::read(entry.path()).unwrap();
			let buffer = Buffer::from(bytes);
			Font::iter(buffer)
		})
		.collect()
}

fn make_source(source: String) -> Source {
	Source::new(SourceId::from_u16(0), FILE_NAME.as_ref(), source)
}

fn get_time() -> time::OffsetDateTime {
	time::OffsetDateTime::now_utc()
}

pub struct WithSource {
	sandbox: Arc<Sandbox>,
	source: Source,
	time: time::OffsetDateTime,
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
			time: get_time(),
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

	fn today(&self, offset: Option<i64>) -> Option<typst::eval::Datetime> {
		// We are in UTC.
		let offset = offset.unwrap_or(0);
		let offset = time::UtcOffset::from_hms(offset.try_into().ok()?, 0, 0).ok()?;
		let time = self.time.checked_to_offset(offset)?;
		Some(typst::eval::Datetime::Date(time.date()))
	}
}
