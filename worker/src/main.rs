use std::io::Write as _;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;

use protocol::{Request, Response};

use crate::render::render;
use crate::sandbox::Sandbox;

mod render;
mod sandbox;

const FILE_NAME: &str = "<user input>";

fn panic_to_string(panic: &dyn std::any::Any) -> String {
	let inner = panic
		.downcast_ref::<&'static str>()
		.copied()
		.or_else(|| panic.downcast_ref::<String>().map(String::as_str))
		.unwrap_or("Box<dyn Any>");
	format!("panicked at '{inner}'")
}

fn write_response(response: &Response) {
	let mut stdout = std::io::stdout().lock();
	bincode::serialize_into(&mut stdout, &response).unwrap();
	stdout.flush().unwrap();
}

/// This can be changed to `&str` by changing the field in the protocol response to a `Cow`,
/// but currently there's no reason to because the string is dynamically formatted anyway.
fn write_progress(msg: String) {
	write_response(&Response::Progress(msg));
}

fn main() {
	let sandbox = Rc::new(Sandbox::new());

	loop {
		let res = bincode::deserialize_from(std::io::stdin().lock());

		if let Err(error) = &res {
			if let bincode::ErrorKind::Io(error) = &**error {
				if error.kind() == std::io::ErrorKind::UnexpectedEof {
					break;
				}
			}
		}

		let request: Request = res.unwrap();

		let response = match request {
			Request::Render { code } => {
				let sandbox = Rc::clone(&sandbox);
				let response = std::panic::catch_unwind(AssertUnwindSafe(move || render(sandbox, code)));
				let response = response
					.map_err(|panic| panic_to_string(&*panic))
					.and_then(|inner| inner);
				Response::Render(response)
			}
			Request::Ast { code } => {
				let ast = typst::syntax::parse(&code);
				Response::Ast(format!("{ast:#?}"))
			}
			Request::Version => Response::Version(protocol::VersionResponse {
				version: env!("TYPST_VERSION").into(),
				git_hash: env!("TYPST_GIT_HASH").into(),
			}),
		};

		comemo::evict(100);

		write_response(&response);
	}
}
