use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::io::Write as _;
use std::ops::Range;

use ariadne::{Cache, Config, Label, Report};
use typst::diag::SourceDiagnostic;
use typst::syntax::FileId;
use typst::World;

use crate::sandbox::WithSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CharIndex {
	first_byte: usize,
	char_index: usize,
}

impl std::ops::Add for CharIndex {
	type Output = CharIndex;

	fn add(self, other: Self) -> Self {
		Self {
			first_byte: self.first_byte + other.first_byte,
			char_index: self.char_index + other.char_index,
		}
	}
}

fn byte_index_to_char_index(source: &str, byte_index: usize) -> CharIndex {
	let mut ret = CharIndex {
		first_byte: 0,
		char_index: 0,
	};

	for ch in source.chars() {
		if byte_index < ret.first_byte + ch.len_utf8() {
			break;
		}
		ret.char_index += 1;
		ret.first_byte += ch.len_utf8();
	}

	ret
}

#[test]
fn test_byte_index_to_char_index() {
	assert_eq!(
		byte_index_to_char_index("abc", 0),
		CharIndex {
			first_byte: 0,
			char_index: 0,
		},
	);
	assert_eq!(
		byte_index_to_char_index("abc", 1),
		CharIndex {
			first_byte: 1,
			char_index: 1,
		},
	);
	assert_eq!(
		byte_index_to_char_index("abc", 2),
		CharIndex {
			first_byte: 2,
			char_index: 2,
		},
	);
	assert_eq!(
		byte_index_to_char_index("abc", 3),
		CharIndex {
			first_byte: 3,
			char_index: 3,
		},
	);
	assert_eq!(
		byte_index_to_char_index("あか", 0),
		CharIndex {
			first_byte: 0,
			char_index: 0,
		},
	);
	assert_eq!(
		byte_index_to_char_index("あか", 3),
		CharIndex {
			first_byte: 3,
			char_index: 1,
		},
	);
	assert_eq!(
		byte_index_to_char_index("あか", 6),
		CharIndex {
			first_byte: 6,
			char_index: 2,
		},
	);
	assert_eq!(
		byte_index_to_char_index("あか", 2),
		CharIndex {
			first_byte: 0,
			char_index: 0,
		},
	);
	assert_eq!(
		byte_index_to_char_index("あか", 7),
		CharIndex {
			first_byte: 6,
			char_index: 2,
		},
	);
}

fn byte_span_to_char_span(source: &str, span: Range<usize>) -> Option<Range<usize>> {
	if span.start > span.end {
		return None;
	}

	let start = byte_index_to_char_index(source, span.start);
	let end = byte_index_to_char_index(&source[start.first_byte..], span.end - span.start) + start;
	Some(start.char_index..end.char_index)
}

#[test]
fn test_byte_span_to_char_span() {
	#![allow(clippy::reversed_empty_ranges)]

	assert_eq!(byte_span_to_char_span("abc", 0..0), Some(0..0));
	assert_eq!(byte_span_to_char_span("abc", 1..2), Some(1..2));
	assert_eq!(byte_span_to_char_span("あか", 0..3), Some(0..1));
	assert_eq!(byte_span_to_char_span("あか", 3..6), Some(1..2));
	assert_eq!(byte_span_to_char_span("あか", 3..3), Some(1..1));
	assert_eq!(byte_span_to_char_span("あか", 2..3), Some(0..0));
	assert_eq!(byte_span_to_char_span("あか", 6..3), None);
}

fn severity_to_report_kind(severity: typst::diag::Severity) -> ariadne::ReportKind<'static> {
	match severity {
		typst::diag::Severity::Error => ariadne::ReportKind::Error,
		typst::diag::Severity::Warning => ariadne::ReportKind::Warning,
	}
}

struct SourceCache<'a> {
	sandbox: &'a WithSource<'a>,
	cache: HashMap<FileId, ariadne::Source>,
}

impl<'a> SourceCache<'a> {
	fn new(sandbox: &'a WithSource) -> Self {
		Self {
			sandbox,
			cache: HashMap::with_capacity(1),
		}
	}
}

impl Cache<FileId> for SourceCache<'_> {
	type Storage = String;

	fn fetch(&mut self, id: &FileId) -> Result<&ariadne::Source, Box<dyn std::fmt::Debug + '_>> {
		let source = match self.cache.entry(*id) {
			Entry::Occupied(entry) => entry.into_mut(),
			Entry::Vacant(entry) => {
				let source = self
					.sandbox
					.source(*id)
					.map_err(|error| Box::new(error) as Box<dyn std::fmt::Debug>)?;
				let source = ariadne::Source::from(source.text().to_owned());
				entry.insert(source)
			}
		};
		Ok(source)
	}

	fn display<'a>(&self, id: &'a FileId) -> Option<Box<dyn std::fmt::Display + 'a>> {
		Some(Box::new(format!("{id:?}")))
	}
}

#[derive(Debug, Clone, Copy)]
struct Span {
	file_id: FileId,
	char_span_start: usize,
	char_span_end: usize,
}

impl ariadne::Span for Span {
	type SourceId = FileId;

	fn source(&self) -> &Self::SourceId {
		&self.file_id
	}

	fn start(&self) -> usize {
		self.char_span_start
	}

	fn end(&self) -> usize {
		self.char_span_end
	}
}

const MAX_LEN: usize = 1950;

pub fn format_diagnostics(sandbox: &WithSource, diagnostics: &[SourceDiagnostic]) -> String {
	let mut cache = SourceCache::new(sandbox);

	let mut bytes = Vec::new();

	let mut diagnostics = diagnostics.iter();
	while let Some(diagnostic) = diagnostics.next() {
		let typst_span = diagnostic.span;
		let span = typst_span.id().map(|file_id| {
			let source = sandbox
				.source(file_id)
				.expect("invalid file ID in diagnostic span");
			let byte_span = source.range(typst_span).unwrap();
			let mut char_span = byte_span_to_char_span(source.text(), byte_span)
				.expect("invalid byte span reported by typst diagnostic");
			// Avoid empty spans.
			if char_span.end == char_span.start {
				char_span.end += 1;
			}
			Span {
				file_id,
				char_span_start: char_span.start,
				char_span_end: char_span.end,
			}
		});

		let report_kind = severity_to_report_kind(diagnostic.severity);
		let source_id = typst_span
			.id()
			.unwrap_or_else(|| sandbox.main_source().id());
		let report_pos = span.map_or(0, |span| span.char_span_start);

		let mut report = Report::build(report_kind, source_id, report_pos)
			.with_config(Config::default().with_tab_width(2).with_color(false))
			.with_message(&diagnostic.message);

		if let Some(span) = span {
			report = report.with_label(Label::new(span));
		}

		if !diagnostic.hints.is_empty() {
			report = report.with_help(diagnostic.hints.join("\n"));
		}

		let report = report.finish();

		let checkpoint = bytes.len();
		// The unwrap will never fail since `Vec`'s `Write` implementation is infallible.
		report.write(&mut cache, &mut bytes).unwrap();

		bytes.push(b'\n');

		if bytes.len() > MAX_LEN {
			bytes.truncate(checkpoint);
			let more = 1 + diagnostics.count();
			let s = if more == 1 { "" } else { "s" };
			write!(bytes, "{more} more diagnostic{} omitted", s).unwrap();
			break;
		}
	}

	// Remove extra spacing newline.
	if bytes.ends_with(b"\n") {
		bytes.pop();
	}

	// The unwrap will never fail since the report is always valid UTF-8.
	String::from_utf8(bytes).unwrap()
}
