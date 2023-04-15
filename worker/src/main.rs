use std::io::Write as _;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

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

fn main() {
	let sandbox = Arc::new(Sandbox::new());

	let mut stdin = std::io::stdin().lock();
	let mut stdout = std::io::stdout().lock();
	loop {
		let res = bincode::deserialize_from(&mut stdin);

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
				let sandbox = Arc::clone(&sandbox);
				let response = std::panic::catch_unwind(AssertUnwindSafe(move || render(sandbox, code)));
				let response = response
					.map_err(|panic| panic_to_string(&*panic))
					.and_then(|inner| inner.map_err(|error| error.to_string()));
				Response::Render(response)
			}
			Request::Ast { code } => {
				let ast = typst::syntax::parse(&code);
				Response::Ast(format!("{ast:#?}"))
			}
		};

		comemo::evict(100);

		bincode::serialize_into(&mut stdout, &response).unwrap();
		stdout.flush().unwrap();
	}
}
