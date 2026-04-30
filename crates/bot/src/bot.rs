use std::collections::{HashSet, VecDeque};
use std::convert::Infallible;
use std::fmt::{Display, Write as _};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::header::{CONTENT_LENGTH, CONTENT_TYPE};
use hyper::http::StatusCode;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use protocol::VersionResponse;
use reqwest::Client;
use ring::hmac;
use rusqlite::{named_params, Connection, OpenFlags};
use serde::Deserialize;
use serde_json::json;
use tokio::join;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};

use crate::worker::Worker;
use crate::SOURCE_URL;

/// U+200D is a zero-width joiner.
/// It prevents the triple backtick from being interpreted as a codeblock but retains ligature support.
const ZERO_WIDTH_JOINER: char = '\u{200D}';
const MAX_SLACK_EVENT_BODY_BYTES: u64 = 1024 * 1024;

type BotError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Prevent garbled output from codeblocks unwittingly terminated by their own content.
fn sanitize_code_block(raw: &str) -> impl Display + '_ {
	struct Helper<'a>(&'a str);

	impl Display for Helper<'_> {
		fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			for section in self.0.split_inclusive("```") {
				let (safe, should_append) = section
					.strip_suffix("```")
					.map_or((section, false), |safe| (safe, true));
				formatter.write_str(safe)?;
				if should_append {
					write!(formatter, "``{ZERO_WIDTH_JOINER}`")?;
				}
			}

			Ok(())
		}
	}

	Helper(raw)
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid theme")]
struct InvalidTheme;

impl FromStr for Theme {
	type Err = InvalidTheme;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(match s {
			"transparent" | "t" => Self::Transparent,
			"light" | "l" => Self::Light,
			"dark" | "d" => Self::Dark,
			_ => return Err(InvalidTheme),
		})
	}
}

#[derive(Default, Debug, Clone, Copy)]
enum Theme {
	Transparent,
	Light,
	#[default]
	Dark,
}

impl Theme {
	const fn preamble(self) -> &'static str {
		match self {
			Self::Transparent => "",
			Self::Light => "#set page(fill: white)\n",
			Self::Dark => concat!(
				"#set page(fill: rgb(248, 248, 248))\n",
				"#set text(fill: rgb(29, 28, 29))\n",
			),
		}
	}
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid page size")]
struct InvalidPageSize;

impl FromStr for PageSize {
	type Err = InvalidPageSize;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(match s {
			"preview" | "p" => Self::Preview,
			"auto" | "a" => Self::Auto,
			"default" | "d" => Self::Default,
			_ => return Err(InvalidPageSize),
		})
	}
}

#[derive(Default, Debug, Clone, Copy)]
enum PageSize {
	#[default]
	Preview,
	Auto,
	Default,
}

impl PageSize {
	const fn preamble(self) -> &'static str {
		match self {
			Self::Preview => "#set page(width: 300pt, height: auto, margin: 10pt)\n",
			Self::Auto => "#set page(width: auto, height: auto, margin: 10pt)\n",
			Self::Default => "",
		}
	}
}

#[derive(Default, Debug, Clone, Copy)]
struct Preamble {
	page_size: PageSize,
	theme: Theme,
}

impl Preamble {
	fn preamble(self) -> String {
		let page_size = self.page_size.preamble();
		let theme = self.theme.preamble();
		if theme.is_empty() && page_size.is_empty() {
			String::new()
		} else {
			format!(
				concat!(
					"// Begin preamble\n",
					"// Page size:\n",
					"{page_size}",
					"// Theme:\n",
					"{theme}",
					"// End preamble\n",
				),
				page_size = page_size,
				theme = theme,
			)
		}
	}
}

#[derive(Debug, Default)]
struct RenderFlags {
	preamble: Preamble,
}

fn render_help() -> String {
	let default_preamble = Preamble::default().preamble();

	format!(
		"\
Render Typst code as an image.

Render command syntax:
- `@typst-bot render [pagesize=<page size>] [theme=<theme>] <code block> [...]`
- Aliases: `@typst-bot r ps={{p,a,d}} t={{d,l,t}} <code block> [...]`

*Flags*
- `pagesize=` (alias: `ps=`):
  - `preview` (alias `p`, default): 10pt margin, 300pt width, auto height
  - `auto` (alias `a`): 10pt margin, auto width, auto height
  - `default` (alias `d`): Leave as Typst's default
- `theme=` (alias `t=`):
  - `dark` (alias `d`, default): Set text and background to match Slack's light theme
  - `light` (alias `l`): Set the background to white
  - `transparent` (alias `t`): Leave the background transparent

To be clear, the full default preamble is:
```
{default_preamble}
```
To remove the preamble entirely, use `pagesize=default theme=transparent`.

*Examples*
```
@typst-bot render `hello, world!`

@typst-bot r ps=a t=l `Short syntax!`

@typst-bot render pagesize=default theme=light ````
= Heading!

And some text.

#lorem(100)
````

@typst-bot render `#myfunc()` I don't understand this code, can anyone help?
```"
	)
}

#[derive(Debug)]
struct CodeBlock {
	source: String,
}

#[derive(Debug, thiserror::Error)]
enum ParseCommandError {
	#[error("missing command")]
	MissingCommand,
	#[error("missing Typst code block")]
	MissingCode,
	#[error("invalid theme")]
	InvalidTheme,
	#[error("invalid page size")]
	InvalidPageSize,
	#[error("unrecognized flag {0:?}")]
	UnrecognizedFlag(String),
}

#[derive(Debug)]
struct ParsedCommand<'a> {
	name: &'a str,
	args: &'a str,
}

fn parse_command(input: &str) -> Result<ParsedCommand<'_>, ParseCommandError> {
	let input = strip_leading_mentions(input).trim();
	let Some((name, args)) = input.split_once(char::is_whitespace) else {
		if input.is_empty() {
			return Err(ParseCommandError::MissingCommand);
		}
		return Ok(ParsedCommand {
			name: input,
			args: "",
		});
	};
	Ok(ParsedCommand {
		name,
		args: args.trim(),
	})
}

fn strip_leading_mentions(mut input: &str) -> &str {
	loop {
		let trimmed = input.trim_start();
		if !trimmed.starts_with("<@") {
			return trimmed;
		}
		let Some(end) = trimmed.find('>') else {
			return trimmed;
		};
		input = &trimmed[end + 1..];
	}
}

fn parse_render_args(input: &str) -> Result<(RenderFlags, CodeBlock), ParseCommandError> {
	let mut flags = RenderFlags::default();
	let mut remaining = input.trim_start();

	while let Some((token, rest)) = split_first_token(remaining) {
		let Some((key, value)) = token.split_once('=') else {
			break;
		};
		match key {
			"theme" | "t" => {
				flags.preamble.theme = value.parse().map_err(|_| ParseCommandError::InvalidTheme)?;
			}
			"pagesize" | "ps" => {
				flags.preamble.page_size = value
					.parse()
					.map_err(|_| ParseCommandError::InvalidPageSize)?;
			}
			_ => return Err(ParseCommandError::UnrecognizedFlag(key.to_owned())),
		}
		remaining = rest.trim_start();
	}

	Ok((flags, extract_code_block(remaining)?))
}

fn split_first_token(input: &str) -> Option<(&str, &str)> {
	let input = input.trim_start();
	if input.is_empty() {
		return None;
	}
	let Some(end) = input.find(char::is_whitespace) else {
		return Some((input, ""));
	};
	Some((&input[..end], &input[end..]))
}

fn extract_code_block(input: &str) -> Result<CodeBlock, ParseCommandError> {
	let input = input.trim_start();
	if input.is_empty() {
		return Err(ParseCommandError::MissingCode);
	}

	if let Some(start) = input.find("```") {
		let code = &input[start + 3..];
		let (language, code) = code
			.split_once('\n')
			.map_or((None, code), |(language, code)| {
				(Some(language.trim()), code)
			});
		let Some(end) = code.find("```") else {
			return Err(ParseCommandError::MissingCode);
		};
		return Ok(CodeBlock {
			source: clean_code_source(&code[..end], language),
		});
	}

	if let Some(start) = input.find('`') {
		let code = &input[start + 1..];
		let Some(end) = code.find('`') else {
			return Err(ParseCommandError::MissingCode);
		};
		return Ok(CodeBlock {
			source: clean_code_source(&code[..end], None),
		});
	}

	Ok(CodeBlock {
		source: clean_code_source(input, None),
	})
}

fn clean_code_source(raw: &str, language: Option<&str>) -> String {
	let mut source = raw.to_owned();
	if language == Some("ansi") {
		source = strip_ansi_escapes::strip_str(source);
	}

	let pattern = format!("`{ZERO_WIDTH_JOINER}`");
	source.replace(&pattern, "``").replace(&pattern, "``")
}

#[derive(serde::Serialize)]
struct TagName(String);

#[derive(Debug, thiserror::Error)]
enum TagNameFromStrError {
	#[error("tag name too long; max is 20 bytes")]
	TooLong,
	#[error("tag name must only contain [a-zA-Z0-9_-]")]
	BadChar,
}

impl Display for TagName {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		self.0.fmt(f)
	}
}

impl FromStr for TagName {
	type Err = TagNameFromStrError;

	fn from_str(raw: &str) -> Result<Self, Self::Err> {
		if raw.len() > 20 {
			return Err(TagNameFromStrError::TooLong);
		}

		let valid_ch = |ch| matches!(ch, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-');
		if !raw.chars().all(valid_ch) {
			return Err(TagNameFromStrError::BadChar);
		}

		Ok(Self(raw.into()))
	}
}

fn interpolate<'a>(template: &str, mut params: impl Iterator<Item = &'a str>) -> String {
	let mut buf = String::with_capacity(template.len() / 2);
	for chunk in template.split("%%") {
		let mut chunks = chunk.split("%s");
		buf += chunks.next().unwrap();
		for chunk in chunks {
			buf += params.next().unwrap_or("%s");
			buf += chunk;
		}
		buf += "%";
	}
	buf.pop();
	buf
}

#[derive(Clone)]
struct SlackClient {
	token: Arc<str>,
	client: Client,
}

impl SlackClient {
	fn new(token: String) -> Self {
		Self {
			token: Arc::from(token),
			client: Client::new(),
		}
	}

	async fn post_message(
		&self,
		target: &SlackTarget,
		text: impl Into<String>,
	) -> Result<(), BotError> {
		let response: SlackApiResponse = self
			.client
			.post("https://slack.com/api/chat.postMessage")
			.bearer_auth(&*self.token)
			.json(&json!({
				"channel": target.channel,
				"thread_ts": target.thread_ts,
				"text": text.into(),
				"unfurl_links": false,
				"unfurl_media": false,
			}))
			.send()
			.await?
			.json()
			.await?;
		response.into_result("chat.postMessage")
	}

	async fn upload_png(
		&self,
		target: &SlackTarget,
		filename: &str,
		image: Vec<u8>,
		initial_comment: Option<String>,
	) -> Result<(), BotError> {
		#[derive(Deserialize)]
		struct UploadUrlResponse {
			ok: bool,
			error: Option<String>,
			upload_url: Option<String>,
			file_id: Option<String>,
		}

		let upload_url_response: UploadUrlResponse = self
			.client
			.post("https://slack.com/api/files.getUploadURLExternal")
			.bearer_auth(&*self.token)
			.json(&json!({
				"filename": filename,
				"length": image.len(),
				"alt_txt": format!("Typst rendered page {filename}"),
			}))
			.send()
			.await?
			.json()
			.await?;

		if !upload_url_response.ok {
			return Err(
				format!(
					"files.getUploadURLExternal failed: {}",
					upload_url_response
						.error
						.unwrap_or_else(|| "unknown error".to_owned())
				)
				.into(),
			);
		}

		let upload_url = upload_url_response
			.upload_url
			.ok_or("files.getUploadURLExternal response did not include upload_url")?;
		let file_id = upload_url_response
			.file_id
			.ok_or("files.getUploadURLExternal response did not include file_id")?;

		let upload_response = self
			.client
			.post(upload_url)
			.header(CONTENT_TYPE.as_str(), "image/png")
			.body(image)
			.send()
			.await?;
		if !upload_response.status().is_success() {
			return Err(
				format!(
					"Slack file upload failed with HTTP {}",
					upload_response.status()
				)
				.into(),
			);
		}

		let response: SlackApiResponse = self
			.client
			.post("https://slack.com/api/files.completeUploadExternal")
			.bearer_auth(&*self.token)
			.json(&json!({
				"channel_id": target.channel,
				"thread_ts": target.thread_ts,
				"initial_comment": initial_comment,
				"files": [{"id": file_id, "title": filename}],
			}))
			.send()
			.await?
			.json()
			.await?;
		response.into_result("files.completeUploadExternal")
	}
}

#[derive(Deserialize)]
struct SlackApiResponse {
	ok: bool,
	error: Option<String>,
}

impl SlackApiResponse {
	fn into_result(self, method: &str) -> Result<(), BotError> {
		if self.ok {
			Ok(())
		} else {
			Err(
				format!(
					"{method} failed: {}",
					self.error.unwrap_or_else(|| "unknown error".to_owned())
				)
				.into(),
			)
		}
	}
}

#[derive(Clone)]
struct SlackTarget {
	channel: String,
	thread_ts: String,
}

struct AppState {
	signing_secret: Arc<str>,
	slack: SlackClient,
	pool: Mutex<Worker>,
	database: std::sync::Mutex<Connection>,
	seen_events: Mutex<SeenEvents>,
	tag_admins: HashSet<String>,
}

struct SeenEvents {
	order: VecDeque<String>,
	set: HashSet<String>,
}

impl SeenEvents {
	const MAX: usize = 2048;

	fn new() -> Self {
		Self {
			order: VecDeque::new(),
			set: HashSet::new(),
		}
	}

	fn insert(&mut self, event_id: String) -> bool {
		if !self.set.insert(event_id.clone()) {
			return false;
		}
		self.order.push_back(event_id);
		while self.order.len() > Self::MAX {
			if let Some(old) = self.order.pop_front() {
				self.set.remove(&old);
			}
		}
		true
	}
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum SlackRequestBody {
	#[serde(rename = "url_verification")]
	UrlVerification { challenge: String },
	#[serde(rename = "event_callback")]
	EventCallback {
		team_id: Option<String>,
		event_id: String,
		event: SlackEvent,
	},
	#[serde(other)]
	Unknown,
}

#[derive(Deserialize)]
struct SlackEvent {
	#[serde(rename = "type")]
	kind: String,
	user: Option<String>,
	text: Option<String>,
	ts: Option<String>,
	channel: Option<String>,
	subtype: Option<String>,
	bot_id: Option<String>,
}

async fn handle_http_request(
	req: Request<Incoming>,
	state: Arc<AppState>,
) -> Result<Response<Full<Bytes>>, Infallible> {
	let response = match (req.method(), req.uri().path()) {
		(&Method::GET, "/healthz") => text_response(StatusCode::OK, "ok"),
		(&Method::POST, "/slack/events") => handle_slack_events(req, state).await,
		_ => text_response(StatusCode::NOT_FOUND, "not found"),
	};

	Ok(response)
}

async fn handle_slack_events(
	req: Request<Incoming>,
	state: Arc<AppState>,
) -> Response<Full<Bytes>> {
	let headers = req.headers().clone();
	if headers
		.get(CONTENT_LENGTH)
		.and_then(|value| value.to_str().ok())
		.and_then(|value| value.parse::<u64>().ok())
		.is_some_and(|length| length > MAX_SLACK_EVENT_BODY_BYTES)
	{
		return text_response(StatusCode::PAYLOAD_TOO_LARGE, "body too large");
	}

	let body = match req.into_body().collect().await {
		Ok(collected) => collected.to_bytes(),
		Err(error) => return text_response(StatusCode::BAD_REQUEST, format!("invalid body: {error}")),
	};
	if body.len() as u64 > MAX_SLACK_EVENT_BODY_BYTES {
		return text_response(StatusCode::PAYLOAD_TOO_LARGE, "body too large");
	}

	if !verify_slack_signature(&headers, &body, &state.signing_secret) {
		return text_response(StatusCode::UNAUTHORIZED, "invalid Slack signature");
	}

	let parsed = match serde_json::from_slice::<SlackRequestBody>(&body) {
		Ok(parsed) => parsed,
		Err(error) => return text_response(StatusCode::BAD_REQUEST, format!("invalid JSON: {error}")),
	};

	match parsed {
		SlackRequestBody::UrlVerification { challenge } => text_response(StatusCode::OK, challenge),
		SlackRequestBody::EventCallback {
			team_id,
			event_id,
			event,
		} => {
			if !state.seen_events.lock().await.insert(event_id) {
				return text_response(StatusCode::OK, "");
			}
			tokio::spawn(async move {
				if let Err(error) = process_event(state, team_id, event).await {
					tracing::error!(?error, "error while processing Slack event");
				}
			});
			text_response(StatusCode::OK, "")
		}
		SlackRequestBody::Unknown => text_response(StatusCode::OK, ""),
	}
}

fn verify_slack_signature(headers: &hyper::HeaderMap, body: &[u8], signing_secret: &str) -> bool {
	let Some(timestamp) = headers
		.get("x-slack-request-timestamp")
		.and_then(|value| value.to_str().ok())
	else {
		return false;
	};
	let Some(signature) = headers
		.get("x-slack-signature")
		.and_then(|value| value.to_str().ok())
	else {
		return false;
	};
	let Ok(timestamp_seconds) = timestamp.parse::<u64>() else {
		return false;
	};
	let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
		return false;
	};
	if now.as_secs().abs_diff(timestamp_seconds) > Duration::from_secs(60 * 5).as_secs() {
		return false;
	}

	let mut base = Vec::with_capacity(3 + timestamp.len() + body.len());
	base.extend_from_slice(b"v0:");
	base.extend_from_slice(timestamp.as_bytes());
	base.extend_from_slice(b":");
	base.extend_from_slice(body);

	let key = hmac::Key::new(hmac::HMAC_SHA256, signing_secret.as_bytes());
	let expected = format!("v0={}", hex_lower(hmac::sign(&key, &base).as_ref()));

	constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

fn hex_lower(bytes: &[u8]) -> String {
	const HEX: &[u8; 16] = b"0123456789abcdef";
	let mut out = String::with_capacity(bytes.len() * 2);
	for &byte in bytes {
		out.push(char::from(HEX[usize::from(byte >> 4)]));
		out.push(char::from(HEX[usize::from(byte & 0x0f)]));
	}
	out
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
	if left.len() != right.len() {
		return false;
	}
	let mut diff = 0;
	for (&left, &right) in left.iter().zip(right) {
		diff |= left ^ right;
	}
	diff == 0
}

async fn process_event(
	state: Arc<AppState>,
	team_id: Option<String>,
	event: SlackEvent,
) -> Result<(), BotError> {
	if event.kind != "app_mention" || event.subtype.is_some() || event.bot_id.is_some() {
		return Ok(());
	}

	let Some(text) = event.text else {
		return Ok(());
	};
	let Some(channel) = event.channel else {
		return Ok(());
	};
	let Some(ts) = event.ts else {
		return Ok(());
	};
	let target = SlackTarget {
		channel,
		thread_ts: ts,
	};
	let scope = format!(
		"{}:{}",
		team_id.as_deref().unwrap_or("unknown-team"),
		target.channel
	);
	let user = event.user.unwrap_or_else(|| "unknown-user".to_owned());

	match process_command(&state, &target, &scope, &user, &text).await {
		Ok(()) => Ok(()),
		Err(error) => {
			state
				.slack
				.post_message(
					&target,
					format!(
						"An error occurred:\n```ansi\n{}```",
						sanitize_code_block(&error.to_string())
					),
				)
				.await
		}
	}
}

async fn process_command(
	state: &AppState,
	target: &SlackTarget,
	scope: &str,
	user: &str,
	text: &str,
) -> Result<(), BotError> {
	let command = parse_command(text)?;
	match command.name {
		"render" | "r" => render(state, target, command.args).await,
		"help" => help(state, target, command.args).await,
		"source" => state.slack.post_message(target, SOURCE_URL).await,
		"ast" => ast(state, target, command.args).await,
		"version" => version(state, target).await,
		#[cfg(feature = "tags")]
		"tag" => tag(state, target, scope, command.args).await,
		#[cfg(feature = "tags")]
		"set-tag" => set_tag(state, target, scope, user, command.args).await,
		#[cfg(feature = "tags")]
		"delete-tag" => delete_tag(state, target, scope, user, command.args).await,
		#[cfg(feature = "tags")]
		"tags" => list_tags(state, target, scope, command.args).await,
		name => {
			state
				.slack
				.post_message(
					target,
					format!("Unknown command `{name}`. Use `@typst-bot help` for available commands."),
				)
				.await
		}
	}
}

async fn render(state: &AppState, target: &SlackTarget, args: &str) -> Result<(), BotError> {
	let (flags, code) = parse_render_args(args)?;
	let mut source = code.source;
	source.insert_str(0, &flags.preamble.preamble());

	let mut progress = String::new();
	let (progress_send, mut progress_recv) = mpsc::channel(4);
	let (res, ()) = {
		let mut pool = state.pool.lock().await;
		join!(pool.render(source, progress_send), async {
			while let Some(item) = progress_recv.recv().await {
				progress.reserve(item.len() + 1);
				progress.push_str(&item);
				progress.push('\n');
				let message = format!("Progress: ```ansi\n{}\n```", sanitize_code_block(&progress));
				_ = state.slack.post_message(target, message).await;
			}
		})
	};

	match res {
		Ok(res) => {
			let mut content = String::new();

			if res.images.is_empty() {
				writeln!(content, "Note: no pages generated")?;
			}

			if res.more_pages > 0 {
				let more_pages = res.more_pages;
				writeln!(
					content,
					"Note: {more_pages} more page{s} ignored",
					s = if more_pages == 1 { "" } else { "s" },
				)?;
			}

			if !res.warnings.is_empty() {
				writeln!(
					content,
					"Render succeeded with warnings:\n```ansi\n{}\n```",
					sanitize_code_block(&res.warnings),
				)?;
			}

			let mut images = res.images.into_iter().enumerate().peekable();
			if images.peek().is_none() {
				state.slack.post_message(target, content.clone()).await?;
			}

			for (i, image) in images {
				let comment = if i == 0 && !content.is_empty() {
					Some(content.clone())
				} else {
					None
				};
				state
					.slack
					.upload_png(target, &format!("page-{}.png", i + 1), image, comment)
					.await?;
			}
		}
		Err(error) => {
			state
				.slack
				.post_message(
					target,
					format!(
						"An error occurred:\n```ansi\n{}\n```",
						sanitize_code_block(&format!("{error:?}"))
					),
				)
				.await?;
		}
	}

	Ok(())
}

async fn help(state: &AppState, target: &SlackTarget, args: &str) -> Result<(), BotError> {
	let response = match args.trim() {
		"" => "\
Commands:
- `@typst-bot render` / `@typst-bot r`: render Typst code
- `@typst-bot ast`: show Typst AST
- `@typst-bot version`: show Typst version
- `@typst-bot source`: show source URL
- `@typst-bot tag`, `set-tag`, `delete-tag`, `tags`: channel-local tags

Use `@typst-bot help render` for render syntax."
			.to_owned(),
		"render" | "r" => render_help(),
		_ => "No detailed help for that command.".to_owned(),
	};
	state.slack.post_message(target, response).await
}

async fn ast(state: &AppState, target: &SlackTarget, args: &str) -> Result<(), BotError> {
	let code = extract_code_block(args)?;
	let res = state.pool.lock().await.ast(code.source).await;

	match res {
		Ok(ast) => {
			let message = format!("```ansi\n{}```", sanitize_code_block(&ast));
			state.slack.post_message(target, message).await?;
		}
		Err(error) => {
			let message = format!(
				"An error occurred:\n```ansi\n{}```",
				sanitize_code_block(&format!("{error:?}")),
			);
			state.slack.post_message(target, message).await?;
		}
	}

	Ok(())
}

async fn version(state: &AppState, target: &SlackTarget) -> Result<(), BotError> {
	let res = state.pool.lock().await.version().await;

	match res {
		Ok(VersionResponse {
			version: typst_version,
		}) => {
			let bot_hash = env!("BUILD_SHA");
			let message = format!("\
The bot was built from git hash <{SOURCE_URL}/tree/{bot_hash}|{bot_hash}>
The bot is using Typst version <https://github.com/typst/typst/releases/v{typst_version}|{typst_version}>\
");
			state.slack.post_message(target, message).await?;
		}
		Err(error) => {
			let message = format!("An error occurred:\n```ansi\n{error}```");
			state.slack.post_message(target, message).await?;
		}
	}

	Ok(())
}

#[cfg(feature = "tags")]
async fn tag(
	state: &AppState,
	target: &SlackTarget,
	scope: &str,
	args: &str,
) -> Result<(), BotError> {
	let mut parts = args.split_whitespace();
	let tag_name = parts.next().ok_or("missing tag name")?;
	let TagName(tag_name) = tag_name.parse()?;
	let parameters = parts.collect::<Vec<_>>();
	let text = state
		.database
		.lock()
		.map_err(|_| "db mutex poisoned")?
		.prepare("select text from slack_tags where name = :name and scope = :scope")?
		.query(named_params!(":name": tag_name, ":scope": scope))?
		.next()?
		.map(|row| row.get::<_, String>("text"))
		.transpose()?;
	let text = text.unwrap_or_else(|| "That tag is not defined.".into());
	let text = interpolate(&text, parameters.into_iter());
	state.slack.post_message(target, text).await
}

#[cfg(feature = "tags")]
async fn set_tag(
	state: &AppState,
	target: &SlackTarget,
	scope: &str,
	user: &str,
	args: &str,
) -> Result<(), BotError> {
	check_tag_admin(state, user)?;
	let (tag_name, tag_text) = args
		.trim()
		.split_once(char::is_whitespace)
		.ok_or("missing tag text")?;
	let TagName(tag_name) = tag_name.parse()?;
	if tag_text.len() > 1000 {
		return Err("tag text too long; max is 1000 bytes".into());
	}

	state
		.database
		.lock()
		.map_err(|_| "db mutex poisoned")?
		.execute(
			"insert into slack_tags (name, scope, text) values (:name, :scope, :text) on conflict do update set text = :text",
			named_params!(":name": tag_name, ":scope": scope, ":text": tag_text),
		)?;

	state
		.slack
		.post_message(target, format!("Tag `{tag_name}` updated by <@{user}>."))
		.await
}

#[cfg(feature = "tags")]
async fn delete_tag(
	state: &AppState,
	target: &SlackTarget,
	scope: &str,
	user: &str,
	args: &str,
) -> Result<(), BotError> {
	check_tag_admin(state, user)?;
	let TagName(tag_name) = args.trim().parse()?;
	let num_rows = state
		.database
		.lock()
		.map_err(|_| "db mutex poisoned")?
		.execute(
			"delete from slack_tags where name = :name and scope = :scope",
			named_params!(":name": tag_name, ":scope": scope),
		)?;

	let message = if num_rows > 0 {
		format!("Tag `{tag_name}` deleted by <@{user}>.")
	} else {
		format!("Tag `{tag_name}` not found.")
	};
	state.slack.post_message(target, message).await
}

#[cfg(feature = "tags")]
async fn list_tags(
	state: &AppState,
	target: &SlackTarget,
	scope: &str,
	args: &str,
) -> Result<(), BotError> {
	let filter = args.trim();
	let filter = (!filter.is_empty()).then_some(filter);
	let reply = {
		let database = state.database.lock().map_err(|_| "db mutex poisoned")?;
		let mut statement = database.prepare(
			"select name from slack_tags where scope = :scope and (:filter is null or instr(name, :filter) > 0) order by name",
		)?;
		let mut results = statement
			.query_map(named_params!(":filter": filter, ":scope": scope), |row| {
				row.get::<_, Box<str>>("name")
			})?;
		results.try_fold(String::new(), |mut acc, name| {
			let name = name?;
			if !acc.is_empty() {
				acc += ", ";
			}
			write!(acc, "`{name}`").unwrap();
			Ok::<_, rusqlite::Error>(acc)
		})?
	};

	let reply = if reply.is_empty() {
		if filter.is_some() {
			"No tags matching that query"
		} else {
			"No tags"
		}
	} else {
		&reply
	};

	state.slack.post_message(target, reply).await
}

#[cfg(feature = "tags")]
fn check_tag_admin(state: &AppState, user: &str) -> Result<(), BotError> {
	if state.tag_admins.is_empty() || state.tag_admins.contains(user) {
		Ok(())
	} else {
		Err("you are not allowed to change tags".into())
	}
}

fn text_response(status: StatusCode, body: impl Into<String>) -> Response<Full<Bytes>> {
	Response::builder()
		.status(status)
		.body(Full::from(Bytes::from(body.into())))
		.unwrap()
}

fn read_secret(env_name: &str, file_env_name: &str) -> String {
	match (std::env::var_os(env_name), std::env::var_os(file_env_name)) {
		(Some(value), None) => value
			.into_string()
			.unwrap_or_else(|_| panic!("`{env_name}` not UTF-8")),
		(None, Some(path)) => {
			std::fs::read_to_string(path).unwrap_or_else(|_| panic!("reading from `{file_env_name}`"))
		}
		(Some(_), Some(_)) => {
			panic!("both `{env_name}` and `{file_env_name}` provided; please only use one")
		}
		(None, None) => panic!("need `{env_name}` or `{file_env_name}` env var"),
	}
	.trim()
	.to_owned()
}

pub async fn run() {
	let token = read_secret("SLACK_BOT_TOKEN", "SLACK_BOT_TOKEN_FILE");
	let signing_secret = read_secret("SLACK_SIGNING_SECRET", "SLACK_SIGNING_SECRET_FILE");
	let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_owned());
	let bind_addr: SocketAddr = bind_addr.parse().expect("`BIND_ADDR` must be host:port");

	let database = Connection::open_with_flags(
		std::env::var_os("DB_PATH").expect("need `DB_PATH` env var"),
		OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
	)
	.unwrap();
	database.execute("create table if not exists slack_tags (name text not null, scope text not null, text text not null, unique (name, scope)) strict", []).unwrap();
	let database = std::sync::Mutex::new(database);

	let pool = Worker::spawn().await.unwrap();
	let tag_admins = std::env::var("SLACK_TAG_ADMIN_USERS")
		.unwrap_or_default()
		.split(',')
		.map(str::trim)
		.filter(|item| !item.is_empty())
		.map(ToOwned::to_owned)
		.collect();

	let state = Arc::new(AppState {
		signing_secret: Arc::from(signing_secret),
		slack: SlackClient::new(token),
		pool: Mutex::new(pool),
		database,
		seen_events: Mutex::new(SeenEvents::new()),
		tag_admins,
	});

	let listener = TcpListener::bind(bind_addr).await.unwrap();
	eprintln!("ready: listening on http://{bind_addr}/slack/events");

	loop {
		let (stream, _) = listener.accept().await.unwrap();
		let state = Arc::clone(&state);
		tokio::spawn(async move {
			let io = TokioIo::new(stream);
			let service = service_fn(move |req| handle_http_request(req, Arc::clone(&state)));
			if let Err(error) = http1::Builder::new().serve_connection(io, service).await {
				tracing::error!(?error, "error serving HTTP connection");
			}
		});
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_mention_command() {
		let parsed = parse_command("<@U123> render `hello`").unwrap();
		assert_eq!(parsed.name, "render");
		assert_eq!(parsed.args, "`hello`");
	}

	#[test]
	fn parses_render_flags_and_code_block() {
		let (flags, code) = parse_render_args("ps=a t=l ```typst\n= Hello\n``` trailing").unwrap();
		assert!(matches!(flags.preamble.page_size, PageSize::Auto));
		assert!(matches!(flags.preamble.theme, Theme::Light));
		assert_eq!(code.source, "= Hello\n");
	}

	#[test]
	fn parses_inline_code() {
		let (_flags, code) = parse_render_args("`hello, world!` extra").unwrap();
		assert_eq!(code.source, "hello, world!");
	}
}
