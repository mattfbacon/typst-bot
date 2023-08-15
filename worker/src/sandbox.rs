use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use comemo::Prehashed;
use typst::diag::{FileError, FileResult, PackageError, PackageResult};
use typst::eval::{Bytes, Library};
use typst::font::{Font, FontBook};
use typst::syntax::{FileId, PackageSpec, Source};

struct FileEntry {
	bytes: Bytes,
	/// This field is filled on demand.
	source: Option<Source>,
}

impl FileEntry {
	fn source(&mut self, id: FileId) -> FileResult<Source> {
		// Fallible `get_or_insert`.
		let source = if let Some(source) = &self.source {
			source
		} else {
			let contents = std::str::from_utf8(&self.bytes).map_err(|_| FileError::InvalidUtf8)?;
			// Defuse the BOM!
			let contents = contents.trim_start_matches('\u{feff}');
			let source = Source::new(id, contents.into());
			self.source.insert(source)
		};
		Ok(source.clone())
	}
}

pub struct Sandbox {
	library: Prehashed<Library>,
	book: Prehashed<FontBook>,
	fonts: Vec<Font>,

	cache_directory: PathBuf,
	http: ureq::Agent,
	files: RefCell<HashMap<FileId, FileEntry>>,
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
}

impl Sandbox {
	pub fn new() -> Self {
		let fonts = fonts();

		Self {
			library: Prehashed::new(typst_library::build()),
			book: Prehashed::new(FontBook::from_fonts(&fonts)),
			fonts,

			cache_directory: std::env::var_os("CACHE_DIRECTORY")
				.expect("need the `CACHE_DIRECTORY` env var")
				.into(),
			http: ureq::Agent::new(),
			files: RefCell::new(HashMap::new()),
		}
	}

	pub fn with_source(self: Arc<Self>, source: String) -> WithSource {
		WithSource {
			sandbox: self,
			source: make_source(source),
			time: get_time(),
		}
	}

	/// Returns the system path of the unpacked package.
	fn ensure_package(&self, package: &PackageSpec) -> PackageResult<PathBuf> {
		let package_subdir = format!("{}/{}/{}", package.namespace, package.name, package.version);
		let path = self.cache_directory.join(package_subdir);

		if path.exists() {
			return Ok(path);
		}

		let url = format!(
			"https://packages.typst.org/{}/{}-{}.tar.gz",
			package.namespace, package.name, package.version,
		);

		let response = self
			.http
			.get(&url)
			.call()
			.map_err(|_| PackageError::NetworkFailed)?;

		let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(response.into_reader()));
		archive.unpack(&path).map_err(|_| {
			_ = std::fs::remove_dir_all(&path);
			PackageError::MalformedArchive
		})?;

		Ok(path)
	}

	fn file(&self, id: FileId) -> FileResult<RefMut<'_, FileEntry>> {
		if let Ok(entry) = RefMut::filter_map(self.files.borrow_mut(), |files| files.get_mut(&id)) {
			return Ok(entry);
		}

		if let Some(package) = id.package() {
			let package_dir = self.ensure_package(package)?;
			// `FileId::path` is always absolute.
			let target = id.path().strip_prefix("/").unwrap();
			let path = package_dir.join(target);
			let contents = std::fs::read(&path).map_err(|error| FileError::from_io(error, &path))?;
			return Ok(RefMut::map(self.files.borrow_mut(), |files| {
				files.entry(id).or_insert(FileEntry {
					bytes: contents.into(),
					source: None,
				})
			}));
		}

		Err(FileError::NotFound(id.path().into()))
	}
}

impl WithSource {
	pub fn source(&self) -> &Source {
		&self.source
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
		} else {
			self.sandbox.file(id)?.source(id)
		}
	}

	fn book(&self) -> &Prehashed<FontBook> {
		&self.sandbox.book
	}

	fn font(&self, id: usize) -> Option<Font> {
		self.sandbox.fonts.get(id).cloned()
	}

	fn file(&self, id: FileId) -> FileResult<Bytes> {
		self.sandbox.file(id).map(|file| file.bytes.clone())
	}

	fn today(&self, offset: Option<i64>) -> Option<typst::eval::Datetime> {
		// We are in UTC.
		let offset = offset.unwrap_or(0);
		let offset = time::UtcOffset::from_hms(offset.try_into().ok()?, 0, 0).ok()?;
		let time = self.time.checked_to_offset(offset)?;
		Some(typst::eval::Datetime::Date(time.date()))
	}
}
