use std::io::Cursor;
use std::num::NonZeroUsize;
use std::sync::Arc;

use typst::diag::SourceError;
use typst::geom::{Color, Size};
use typst::syntax::{ErrorPos, Source};

use crate::sandbox::Sandbox;
use crate::FILE_NAME;

const DESIRED_RESOLUTION: f32 = 1000.0;
const MAX_SIZE: f32 = 1000.0;

#[derive(Debug, thiserror::Error)]
#[error("rendered output was too big")]
pub struct TooBig;

fn determine_pixels_per_point(size: Size) -> Result<f32, TooBig> {
	// We want to truncate.
	#![allow(clippy::cast_possible_truncation)]

	let x = size.x.to_pt() as f32;
	let y = size.y.to_pt() as f32;

	if x > MAX_SIZE || y > MAX_SIZE {
		Err(TooBig)
	} else {
		let area = x * y;
		Ok(DESIRED_RESOLUTION / area.sqrt())
	}
}

#[derive(Debug)]
pub struct SourceErrorsWithSource {
	source: Source,
	errors: Vec<SourceError>,
}

impl std::fmt::Display for SourceErrorsWithSource {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		use ariadne::{Config, Label, Report};

		struct SourceCache(ariadne::Source);

		impl ariadne::Cache<()> for SourceCache {
			fn fetch(&mut self, _id: &()) -> Result<&ariadne::Source, Box<dyn std::fmt::Debug + '_>> {
				Ok(&self.0)
			}

			fn display<'a>(&self, _id: &'a ()) -> Option<Box<dyn std::fmt::Display + 'a>> {
				Some(Box::new(FILE_NAME))
			}
		}

		let mut cache = SourceCache(ariadne::Source::from(self.source.text()));

		let mut bytes = Vec::new();

		for error in &self.errors {
			bytes.clear();

			let span = self.source.range(error.span);
			let span = match error.pos {
				ErrorPos::Full => span,
				ErrorPos::Start => span.start..span.start,
				ErrorPos::End => span.end..span.end,
			};

			let report = Report::build(ariadne::ReportKind::Error, (), 0)
				.with_config(Config::default().with_tab_width(2).with_color(false))
				.with_message(&error.message)
				.with_label(Label::new(span))
				.finish();
			// The unwrap will never fail since `Vec`'s `Write` implementation is infallible.
			report.write(&mut cache, &mut bytes).unwrap();

			// The unwrap will never fail since the output string is always valid UTF-8.
			formatter.write_str(std::str::from_utf8(&bytes).unwrap())?;
		}

		Ok(())
	}
}

impl std::error::Error for SourceErrorsWithSource {}

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Source(#[from] SourceErrorsWithSource),
	#[error(transparent)]
	TooBig(#[from] TooBig),
	#[error("no pages in rendered output")]
	NoPages,
}

pub struct Output {
	pub image: Vec<u8>,
	pub more_pages: Option<NonZeroUsize>,
}

pub fn render(sandbox: Arc<Sandbox>, fill: Color, source: String) -> Result<Output, Error> {
	let world = sandbox.with_source(source);

	let document = typst::compile(&world).map_err(|errors| SourceErrorsWithSource {
		source: world.into_source(),
		errors: *errors,
	})?;
	let frame = &document.pages.get(0).ok_or(Error::NoPages)?;
	let more_pages = NonZeroUsize::new(document.pages.len().saturating_sub(1));

	let pixels_per_point = determine_pixels_per_point(frame.size())?;

	let pixmap = typst::export::render(frame, pixels_per_point, fill);

	let mut writer = Cursor::new(Vec::new());

	// The unwrap will never fail since `Vec`'s `Write` implementation is infallible.
	image::write_buffer_with_format(
		&mut writer,
		bytemuck::cast_slice(pixmap.pixels()),
		pixmap.width(),
		pixmap.height(),
		image::ColorType::Rgba8,
		image::ImageFormat::Png,
	)
	.unwrap();

	let image = writer.into_inner();
	Ok(Output { image, more_pages })
}
