use std::pin::pin;
use std::process::{Child, Stdio};
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _};
use protocol::{Request, Response};
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::Instant;

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

	async fn run(
		&mut self,
		request: Request,
		progress_channel_outer: Option<mpsc::Sender<String>>,
	) -> anyhow::Result<Response> {
		struct Timeout;

		// This timeout is reset any time a progress message is received.
		let fast_timeout = Duration::from_secs(5);
		// This is a universal timeout that is never reset.
		let long_timeout = Duration::from_secs(30);
		let mut tries_left = 2;

		loop {
			let (progress_inner_send, mut progress_inner_recv) = mpsc::channel(1);

			let res = {
				let mut fut = pin!(self
					.process
					.communicate(request.clone(), Some(progress_inner_send)));
				let mut fast_timeout_fut = pin!(tokio::time::sleep(fast_timeout));
				let mut long_timeout_fut = pin!(tokio::time::sleep(long_timeout));
				loop {
					select! {
						res = fut.as_mut() => {
							break Ok(res);
						}
						Some(progress) = progress_inner_recv.recv() => {
							fast_timeout_fut.as_mut().reset(Instant::now() + fast_timeout);
							if let Some(outer) = &progress_channel_outer {
								_ = outer.send(progress).await;
							}
						}
						() = fast_timeout_fut.as_mut() => {
							break Err(Timeout);
						}
						() = long_timeout_fut.as_mut() => {
							break Err(Timeout);
						}
					};
				}
			};

			let error = match res {
				Ok(Ok(response)) => return Ok(response),
				Ok(Err(error)) => {
					self.process.replace().await?;
					error
				}
				Err(Timeout) => {
					self.process.replace().await?;
					bail!("timeout");
				}
			};

			tries_left -= 1;
			if tries_left == 0 {
				return Err(error);
			}
		}
	}

	pub async fn render(
		&mut self,
		code: String,
		progress_channel: mpsc::Sender<String>,
	) -> anyhow::Result<protocol::Rendered> {
		let response = self
			.run(Request::Render { code }, Some(progress_channel))
			.await?;
		let Response::Render(response) = response else {
			bail!("expected Render response, got {response:?}");
		};
		response.map_err(|error| anyhow!(error))
	}

	pub async fn ast(&mut self, code: String) -> anyhow::Result<protocol::AstResponse> {
		let response = self.run(Request::Ast { code }, None).await?;
		let Response::Ast(response) = response else {
			bail!("expected Ast response, got {response:?}");
		};
		Ok(response)
	}

	pub async fn version(&mut self) -> anyhow::Result<protocol::VersionResponse> {
		let response = self.run(Request::Version, None).await?;
		let Response::Version(response) = response else {
			bail!("expected Version response, got {response:?}");
		};
		Ok(response)
	}
}

#[derive(Debug)]
struct Process {
	child: Option<Child>,
}

impl Process {
	async fn spawn() -> anyhow::Result<Self> {
		const VAR_NAME: &str = "TYPST_BOT_WORKER_PATH";
		let worker_path = std::env::var_os(VAR_NAME).unwrap_or_else(|| "./worker".into());
		#[allow(clippy::unnecessary_debug_formatting)]
		let child = std::process::Command::new(&worker_path)
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::inherit())
			.spawn()
			.with_context(|| format!("spawning worker process (path={worker_path:?}).\n\ntry setting the env var {VAR_NAME} to point to the worker binary, e.g. in the cargo target directory. alternatively, follow the instructions in the README that describe how to set up a standalone installation."))?;

		let mut ret = Self { child: Some(child) };
		// Ask for the version and ignore it, as a health check.
		ret
			.communicate(Request::Version, None)
			.await
			.context("initial health check")?;

		Ok(ret)
	}

	async fn replace(&mut self) -> anyhow::Result<()> {
		let new = Self::spawn().await?;
		let old = std::mem::replace(self, new);
		tokio::task::spawn_blocking(move || {
			if let Some(mut child) = old.child {
				_ = child.kill();
				_ = child.wait();
			}
		})
		.await
		.context("joining kill task")?;
		Ok(())
	}

	async fn communicate(
		&mut self,
		request: Request,
		progress_channel: Option<mpsc::Sender<String>>,
	) -> anyhow::Result<Response> {
		let mut child = self.child.take().unwrap();
		let (child, res) = tokio::task::spawn_blocking(move || {
			fn inner(
				child: &mut Child,
				request: &Request,
				progress_channel: Option<&mpsc::Sender<String>>,
			) -> bincode::Result<Response> {
				bincode::serialize_into(child.stdin.as_mut().unwrap(), &request)?;
				loop {
					let response: Response = bincode::deserialize_from(child.stdout.as_mut().unwrap())?;

					if let Response::Progress(progress) = response {
						if let Some(chan) = &progress_channel {
							_ = chan.blocking_send(progress);
						}
					} else {
						break Ok(response);
					}
				}
			}
			let res = inner(&mut child, &request, progress_channel.as_ref());
			(child, res)
		})
		.await
		.context("joining communication task")?;
		self.child = Some(child);
		res.context("communicating with worker")
	}
}
