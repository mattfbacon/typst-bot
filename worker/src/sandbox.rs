use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use comemo::Prehashed;
use typst::diag::{FileError, FileResult};
use typst::eval::{Bytes, Library};
use typst::font::{Font, FontBook};
use typst::syntax::{FileId, Source};
use typst::util::PathExt;

use crate::file::{PathHash, PathSlot};
use crate::package::prepare_package;

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
			let buffer = Bytes::from(bytes);
			Font::iter(buffer)
		})
		.collect()
}

fn make_source(source: String) -> Source {
	Source::detached(source)
}

fn get_time() -> time::OffsetDateTime {
	time::OffsetDateTime::now_utc()
}

pub struct WithSource {
	sandbox: Arc<Sandbox>,
	source: Source,
	time: time::OffsetDateTime,
	hashes: RefCell<HashMap<FileId, FileResult<PathHash>>>,
	paths: RefCell<HashMap<PathHash, PathSlot>>,
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
			hashes: RefCell::default(),
			paths: RefCell::default(),
		}
	}
}

impl WithSource {
	pub fn source(&self) -> &Source {
		&self.source
	}

	fn slot(&self, id: FileId) -> FileResult<RefMut<PathSlot>> {
		let mut system_path = PathBuf::new();
		let hash = self
			.hashes
			.borrow_mut()
			.entry(id)
			.or_insert_with(|| {
				// Determine the root path relative to which the file path
				// will be resolved.
				let root = match id.package() {
					Some(spec) => prepare_package(spec)?,
					None => PathBuf::from("."),
				};

				// Join the path to the root. If it tries to escape, deny
				// access. Note: It can still escape via symlinks.
				system_path = root.join_rooted(id.path()).ok_or(FileError::AccessDenied)?;

				PathHash::new(&system_path)
			})
			.clone()?;

		Ok(RefMut::map(self.paths.borrow_mut(), |paths| {
			paths
				.entry(hash)
				// This will only trigger if the `or_insert_with` above also
				// triggered.
				.or_insert_with(|| PathSlot::new(id, system_path))
		}))
	}
}

impl typst::World for WithSource {
	fn library(&self) -> &Prehashed<Library> {
		&self.sandbox.library
	}

	fn main(&self) -> Source {
		self.source.clone()
	}

	fn source(&self, id: FileId) -> FileResult<Source> {
		if id == self.source.id() {
			Ok(self.source.clone())
		} else if id.package().is_some() {
			self.slot(id)?.source()
		} else {
			Err(FileError::NotFound(id.path().into()))
		}
	}

	fn book(&self) -> &Prehashed<FontBook> {
		&self.sandbox.book
	}

	fn font(&self, id: usize) -> Option<Font> {
		self.sandbox.fonts.get(id).cloned()
	}

	fn file(&self, id: FileId) -> FileResult<Bytes> {
		if id.package().is_some() {
			self.slot(id)?.file()
		} else {
			Err(FileError::NotFound(id.path().into()))
		}
	}

	fn today(&self, offset: Option<i64>) -> Option<typst::eval::Datetime> {
		// We are in UTC.
		let offset = offset.unwrap_or(0);
		let offset = time::UtcOffset::from_hms(offset.try_into().ok()?, 0, 0).ok()?;
		let time = self.time.checked_to_offset(offset)?;
		Some(typst::eval::Datetime::Date(time.date()))
	}
}
