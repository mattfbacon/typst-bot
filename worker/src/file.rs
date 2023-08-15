use std::cell::OnceCell;
use std::fs;
use std::hash::Hash;
use std::path::{Path, PathBuf};

use same_file::Handle;
use siphasher::sip128::{Hasher128, SipHasher13};
use typst::diag::{FileError, FileResult};
use typst::eval::Bytes;
use typst::syntax::{FileId, Source};

/// Holds canonical data for all paths pointing to the same entity.
///
/// Both fields can be populated if the file is both imported and read().
pub struct PathSlot {
	/// The slot's canonical file id.
	id: FileId,
	/// The slot's path on the system.
	system_path: PathBuf,
	/// The lazily loaded source file for a path hash.
	source: OnceCell<FileResult<Source>>,
	/// The lazily loaded buffer for a path hash.
	buffer: OnceCell<FileResult<Bytes>>,
}

impl PathSlot {
	pub fn new(id: FileId, system_path: PathBuf) -> Self {
		Self {
			id,
			system_path,
			source: OnceCell::new(),
			buffer: OnceCell::new(),
		}
	}
	pub fn source(&self) -> FileResult<Source> {
		self
			.source
			.get_or_init(|| {
				let buf = read(&self.system_path)?;
				let text = decode_utf8(buf)?;
				Ok(Source::new(self.id, text))
			})
			.clone()
	}

	pub fn file(&self) -> FileResult<Bytes> {
		self
			.buffer
			.get_or_init(|| read(&self.system_path).map(Bytes::from))
			.clone()
	}
}

/// A hash that is the same for all paths pointing to the same entity.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct PathHash(u128);

impl PathHash {
	pub fn new(path: &Path) -> FileResult<Self> {
		let f = |e| FileError::from_io(e, path);
		let handle = Handle::from_path(path).map_err(f)?;
		let mut state = SipHasher13::new();
		handle.hash(&mut state);
		Ok(Self(state.finish128().as_u128()))
	}
}

/// Read a file.
fn read(path: &Path) -> FileResult<Vec<u8>> {
	let f = |e| FileError::from_io(e, path);
	if fs::metadata(path).map_err(f)?.is_dir() {
		Err(FileError::IsDirectory)
	} else {
		fs::read(path).map_err(f)
	}
}

/// Decode UTF-8 with an optional BOM.
fn decode_utf8(buf: Vec<u8>) -> FileResult<String> {
	Ok(if buf.starts_with(b"\xef\xbb\xbf") {
		// Remove UTF-8 BOM.
		std::str::from_utf8(&buf[3..])?.into()
	} else {
		// Assume UTF-8.
		String::from_utf8(buf)?
	})
}
