use std::io::ErrorKind;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::time::Duration;

use protocol::{Request, Response};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
struct Command {
	response: oneshot::Sender<Result<Response, String>>,
	request: Request,
}

#[derive(Debug)]
pub struct Worker {
	send: mpsc::Sender<Command>,
}

impl Worker {
	pub fn spawn() -> Self {
		let (send, recv) = mpsc::channel(8);
		tokio::task::spawn(in_thread(recv));
		Self { send }
	}

	async fn run(&self, request: Request) -> Result<Response, String> {
		let (send_ret, recv_ret) = oneshot::channel();
		self
			.send
			.send(Command {
				response: send_ret,
				request,
			})
			.await
			.unwrap();
		recv_ret.await.unwrap()
	}

	pub async fn render(&self, code: String) -> protocol::RenderResponse {
		let response = self.run(Request::Render { code }).await;
		let Response::Render(response) = response? else { unreachable!() };
		response
	}

	pub async fn ast(&self, code: String) -> Result<protocol::AstResponse, String> {
		let response = self.run(Request::Ast { code }).await;
		let Response::Ast(response) = response? else { unreachable!() };
		Ok(response)
	}
}

struct Process {
	child: Child,
	io: Option<(ChildStdin, ChildStdout)>,
}

impl Process {
	fn spawn() -> Self {
		let mut child = std::process::Command::new("./worker")
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.spawn()
			.unwrap();
		let stdin = child.stdin.take().unwrap();
		let stdout = child.stdout.take().unwrap();
		Self {
			io: Some((stdin, stdout)),
			child,
		}
	}

	async fn replace(&mut self) {
		let new = Self::spawn();
		let mut old = std::mem::replace(self, new);
		tokio::task::spawn_blocking(move || {
			_ = old.child.kill();
			_ = old.child.wait();
		})
		.await
		.unwrap();
	}

	async fn communicate(&mut self, request: Request) -> std::io::Result<Response> {
		let (mut stdin, mut stdout) = self.io.take().unwrap();
		let (stdin, stdout, res) = tokio::task::spawn_blocking(move || {
			fn inner(
				stdin: &mut ChildStdin,
				stdout: &mut ChildStdout,
				request: &Request,
			) -> bincode::Result<Response> {
				bincode::serialize_into(stdin, &request)?;
				bincode::deserialize_from(stdout)
			}
			let res = inner(&mut stdin, &mut stdout, &request);
			(stdin, stdout, res)
		})
		.await
		.unwrap();
		self.io = Some((stdin, stdout));
		res.map_err(|error| match *error {
			bincode::ErrorKind::Io(io) => io,
			_ => ErrorKind::InvalidData.into(),
		})
	}
}

async fn in_thread(mut recv: mpsc::Receiver<Command>) {
	let timeout = Duration::from_secs(1);

	let mut process = Process::spawn();

	while let Some(command) = recv.recv().await {
		let res = tokio::time::timeout(timeout, process.communicate(command.request)).await;
		let response = match res {
			Err(_timeout) => {
				process.replace().await;
				Err("timeout".into())
			}
			Ok(Err(io)) => Err(io.to_string()),
			Ok(Ok(response)) => Ok(response),
		};
		_ = command.response.send(response);
	}
}
