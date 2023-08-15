use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Context as _};
use protocol::{Request, Response};

#[derive(Debug)]
pub struct Worker {
	process: Process,
}

impl Worker {
	pub async fn spawn() -> anyhow::Result<Self> {
		Ok(Self {
			process: Process::spawn().await?,
		})
	}

	async fn run(&mut self, request: Request) -> anyhow::Result<Response> {
		let timeout = Duration::from_secs(5);
		let mut tries_left = 2;

		loop {
			let res = tokio::time::timeout(timeout, self.process.communicate(request.clone())).await;

			let error = match res {
				Ok(res @ Ok(..)) => break res,
				Ok(Err(error)) => {
					self.process.replace().await?;
					error
				}
				Err(_timeout) => {
					self.process.replace().await?;
					break Err(anyhow!("timeout"));
				}
			};

			tries_left -= 1;
			if tries_left == 0 {
				break Err(error);
			}
		}
	}

	pub async fn render(&mut self, code: String) -> anyhow::Result<protocol::Rendered> {
		let response = self.run(Request::Render { code }).await;
		let Response::Render(response) = response? else {
			unreachable!()
		};
		response.map_err(|error| anyhow!(error))
	}

	pub async fn ast(&mut self, code: String) -> anyhow::Result<protocol::AstResponse> {
		let response = self.run(Request::Ast { code }).await;
		let Response::Ast(response) = response? else {
			unreachable!()
		};
		Ok(response)
	}

	pub async fn version(&mut self) -> anyhow::Result<protocol::VersionResponse> {
		let response = self.run(Request::Version).await;
		let Response::Version(response) = response? else {
			unreachable!()
		};
		Ok(response)
	}
}

#[derive(Debug)]
struct Process {
	child: Child,
	io: Option<(ChildStdin, ChildStdout)>,
}

impl Process {
	async fn spawn() -> anyhow::Result<Self> {
		let mut child = std::process::Command::new("./worker")
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.spawn()
			.context("spawning worker process.\n\nthis is likely because you are trying to run the bot from a checkout of the repo and `worker` is a directory. you can fix this by changing the path to the worker binary to point to the worker binary in the cargo target directory. alternatively, follow the instructions in the README that describe how to set up a standalone installation.")?;

		let stdin = child.stdin.take().unwrap();
		let stdout = child.stdout.take().unwrap();

		let mut ret = Self {
			io: Some((stdin, stdout)),
			child,
		};
		// Ask for the version and ignore it, as a health check.
		ret
			.communicate(Request::Version)
			.await
			.context("initial health check")?;

		Ok(ret)
	}

	async fn replace(&mut self) -> anyhow::Result<()> {
		let new = Self::spawn().await?;
		let mut old = std::mem::replace(self, new);
		tokio::task::spawn_blocking(move || {
			_ = old.child.kill();
			_ = old.child.wait();
		})
		.await
		.context("joining kill task")?;
		Ok(())
	}

	async fn communicate(&mut self, request: Request) -> anyhow::Result<Response> {
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
		.context("joining communication task")?;
		self.io = Some((stdin, stdout));
		res.context("communicating with worker")
	}
}
