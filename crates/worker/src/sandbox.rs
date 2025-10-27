use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use typst::diag::{eco_format, FileError, FileResult, PackageError, PackageResult};
use typst::foundations::{Bytes, Datetime};
use typst::syntax::package::PackageSpec;
use typst::syntax::{FileId, Source};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt as _};

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
	library: LazyHash<Library>,
	book: LazyHash<FontBook>,
	fonts: Vec<Font>,

	cache_directory: PathBuf,
	http: ureq::Agent,
	files: Mutex<HashMap<FileId, FileEntry>>,
}

fn fonts() -> Vec<Font> {
	typst_assets::fonts()
		.chain(typst_dev_assets::fonts())
		.flat_map(|bytes| {
			let buffer = Bytes::new(bytes);
			let face_count = ttf_parser::fonts_in_collection(&buffer).unwrap_or(1);
			(0..face_count).map(move |face| {
				Font::new(buffer.clone(), face).expect("failed to load font from typst-assets")
			})
		})
		.collect()
}

fn make_source(source: String) -> Source {
	Source::detached(source)
}

fn get_time() -> time::OffsetDateTime {
	time::OffsetDateTime::now_utc()
}

fn retry<T, E>(mut f: impl FnMut() -> Result<T, E>) -> Result<T, E> {
	if let Ok(ok) = f() {
		Ok(ok)
	} else {
		f()
	}
}

pub struct WithSource<'a> {
	sandbox: &'a Sandbox,
	source: Source,
	time: time::OffsetDateTime,
}

impl Sandbox {
	pub fn new() -> Self {
		let fonts = fonts();

		Self {
			library: LazyHash::new(Library::default()),
			book: LazyHash::new(FontBook::from_fonts(&fonts)),
			fonts,

			cache_directory: std::env::var_os("CACHE_DIRECTORY")
				.expect("need the `CACHE_DIRECTORY` env var")
				.into(),
			http: ureq::agent(),
			files: Mutex::new(HashMap::new()),
		}
	}

	pub fn with_source(&self, source: String) -> WithSource<'_> {
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

		eprintln!("downloading {package}");
		crate::write_progress(format!("downloading {package}"));

		let url = format!(
			"https://packages.typst.org/{}/{}-{}.tar.gz",
			package.namespace, package.name, package.version,
		);

		let response = retry(|| {
			let response = self
				.http
				.get(&url)
				.call()
				.map_err(|error| eco_format!("{error}"))?;

			let status = response.status();
			if !status.is_success() {
				return Err(eco_format!(
					"response returned unsuccessful status code {status}",
				));
			}

			Ok(response)
		})
		.map_err(|error| PackageError::NetworkFailed(Some(error)))?;

		let compressed_archive = response
			.into_body()
			.read_to_vec()
			.map_err(|error| PackageError::NetworkFailed(Some(eco_format!("{error}"))))?;
		let raw_archive = zune_inflate::DeflateDecoder::new(&compressed_archive)
			.decode_gzip()
			.map_err(|error| PackageError::MalformedArchive(Some(eco_format!("{error}"))))?;
		let mut archive = tar::Archive::new(raw_archive.as_slice());
		archive.unpack(&path).map_err(|error| {
			_ = std::fs::remove_dir_all(&path);
			PackageError::MalformedArchive(Some(eco_format!("{error}")))
		})?;

		Ok(path)
	}

	// Weird pattern because mapping a MutexGuard is not stable yet.
	fn file<T>(&self, id: FileId, map: impl FnOnce(&mut FileEntry) -> T) -> FileResult<T> {
		let mut files = self.files.lock().unwrap();
		if let Some(entry) = files.get_mut(&id) {
			return Ok(map(entry));
		}
		// `files` must stay locked here so we don't download the same package multiple times.
		// TODO proper multithreading, maybe with typst-kit.

		'x: {
			if let Some(package) = id.package() {
				let package_dir = self.ensure_package(package)?;
				let Some(path) = id.vpath().resolve(&package_dir) else {
					break 'x;
				};
				let contents = std::fs::read(&path).map_err(|error| FileError::from_io(error, &path))?;
				let entry = files.entry(id).or_insert(FileEntry {
					bytes: Bytes::new(contents),
					source: None,
				});
				return Ok(map(entry));
			}
		}

		Err(FileError::NotFound(id.vpath().as_rootless_path().into()))
	}
}

impl WithSource<'_> {
	pub fn main_source(&self) -> &Source {
		&self.source
	}
}

impl typst::World for WithSource<'_> {
	fn library(&self) -> &LazyHash<Library> {
		&self.sandbox.library
	}

	fn main(&self) -> FileId {
		self.source.id()
	}

	fn source(&self, id: FileId) -> FileResult<Source> {
		if id == self.source.id() {
			Ok(self.source.clone())
		} else {
			self.sandbox.file(id, |file| file.source(id))?
		}
	}

	fn book(&self) -> &LazyHash<FontBook> {
		&self.sandbox.book
	}

	fn font(&self, id: usize) -> Option<Font> {
		self.sandbox.fonts.get(id).cloned()
	}

	fn file(&self, id: FileId) -> FileResult<Bytes> {
		self.sandbox.file(id, |file| file.bytes.clone())
	}

	fn today(&self, offset: Option<i64>) -> Option<Datetime> {
		// We are in UTC.
		let offset = offset.unwrap_or(0);
		let offset = time::UtcOffset::from_hms(offset.try_into().ok()?, 0, 0).ok()?;
		let time = self.time.checked_to_offset(offset)?;
		Some(Datetime::Date(time.date()))
	}
}
