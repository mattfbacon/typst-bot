use std::io::Cursor;
use std::num::NonZeroUsize;

use protocol::Rendered;
use typst::eval::Tracer;
use typst::layout::{Axis, Size};
use typst::visualize::Color;

use crate::diagnostic::format_diagnostics;
use crate::sandbox::Sandbox;

const DESIRED_RESOLUTION: f32 = 1000.0;
const MAX_SIZE: f32 = 10000.0;
const MAX_PIXELS_PER_POINT: f32 = 5.0;

#[derive(Debug, thiserror::Error)]
#[error(
	"rendered output was too big: the {axis:?} axis was {size} pt but the maximum is {MAX_SIZE}"
)]
pub struct TooBig {
	size: f32,
	axis: Axis,
}

fn determine_pixels_per_point(size: Size) -> Result<f32, TooBig> {
	// We want to truncate.
	#![allow(clippy::cast_possible_truncation)]

	let x = size.x.to_pt() as f32;
	let y = size.y.to_pt() as f32;

	if x > MAX_SIZE {
		Err(TooBig {
			size: x,
			axis: Axis::X,
		})
	} else if y > MAX_SIZE {
		Err(TooBig {
			size: y,
			axis: Axis::Y,
		})
	} else {
		let area = x * y;
		let nominal = DESIRED_RESOLUTION / area.sqrt();
		Ok(nominal.min(MAX_PIXELS_PER_POINT))
	}
}

fn to_string(v: impl ToString) -> String {
	v.to_string()
}

pub fn render(sandbox: &Sandbox, source: String) -> Result<Rendered, String> {
	let world = sandbox.with_source(source);

	let mut tracer = Tracer::default();
	let document =
		typst::compile(&world, &mut tracer).map_err(|diags| format_diagnostics(&world, &diags))?;
	let warnings = tracer.warnings();

	let frame = &document.pages.get(0).ok_or("no pages in rendered output")?;
	let more_pages = NonZeroUsize::new(document.pages.len().saturating_sub(1));

	let pixels_per_point = determine_pixels_per_point(frame.size()).map_err(to_string)?;

	let transparent = Color::from_u8(0, 0, 0, 0);
	let pixmap = typst_render::render(frame, pixels_per_point, transparent);

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
	Ok(Rendered {
		image,
		more_pages,
		warnings: format_diagnostics(&world, &warnings),
	})
}
