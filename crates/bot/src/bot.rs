use std::collections::HashMap;
use std::fmt::{Display, Write as _};
use std::str::FromStr;

use poise::serenity_prelude::GatewayIntents;
use poise::{async_trait, CreateReply};
use protocol::VersionResponse;
use rusqlite::{named_params, Connection, OpenFlags};
use serenity::builder::{CreateAllowedMentions, CreateAttachment};
use tokio::join;
use tokio::sync::{mpsc, Mutex};

use crate::worker::Worker;
use crate::SOURCE_URL;

/// U+200D is a zero-width joiner.
/// It prevents the triple backtick from being interpreted as a codeblock but retains ligature support.
const ZERO_WIDTH_JOINER: char = '\u{200D}';

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
				"#set page(fill: rgb(49, 51, 56))\n",
				"#set text(fill: rgb(219, 222, 225))\n",
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

struct Data {
	pool: Mutex<Worker>,
	database: std::sync::Mutex<Connection>,
}

type PoiseError = Box<dyn std::error::Error + Send + Sync + 'static>;
type Context<'a> = poise::Context<'a, Data, PoiseError>;

#[derive(Debug, Default)]
struct RenderFlags {
	preamble: Preamble,
}

#[async_trait]
impl<'a> poise::PopArgument<'a> for RenderFlags {
	async fn pop_from(
		args: &'a str,
		attachment_index: usize,
		ctx: &serenity::prelude::Context,
		message: &poise::serenity_prelude::Message,
	) -> Result<(&'a str, usize, Self), (PoiseError, Option<String>)> {
		fn inner(raw: &HashMap<String, String>) -> Result<RenderFlags, PoiseError> {
			let mut parsed = RenderFlags::default();

			for (key, value) in raw {
				match key.as_str() {
					"theme" | "t" => {
						parsed.preamble.theme = value.parse().map_err(|_| "invalid theme")?;
					}
					"pagesize" | "ps" => {
						parsed.preamble.page_size = value.parse().map_err(|_| "invalid page size")?;
					}
					_ => {
						return Err(format!("unrecognized flag {key:?}").into());
					}
				}
			}

			Ok(parsed)
		}

		let (remaining, pos, raw) =
			poise::prefix_argument::KeyValueArgs::pop_from(args, attachment_index, ctx, message).await?;

		inner(&raw.0)
			.map(|parsed| (remaining, pos, parsed))
			.map_err(|error| (error, None))
	}
}

fn render_help() -> String {
	let default_preamble = Preamble::default().preamble();

	format!(
		"\
Render Typst code as an image.

Syntax: `?render [pagesize=<page size>] [theme=<theme>] <code block> [...]`

**Flags**
- `pagesize` can be `preview` (default),  `auto`, or `default`.
- `theme` can be `dark` (default), `light`, or `transparent`.

To be clear, the full default preamble is:
```
{default_preamble}
```
To remove the preamble entirely, use `pagesize=default theme=transparent`.

**Examples**
```
?render `hello, world!`

?render pagesize=default theme=light ``‍`
= Heading!

And some text.

#lorem(100)
``‍`

?render `#myfunc()` I don't understand this code, can anyone help?
```"
	)
}

/// Extracts the contents of a code block.
///
/// If the language is `ansi`, then ANSI escape codes will be stripped from the input.
struct CodeBlock {
	source: String,
}

#[async_trait]
impl<'a> poise::PopArgument<'a> for CodeBlock {
	async fn pop_from(
		mut args: &'a str,
		attachment_index: usize,
		ctx: &serenity::prelude::Context,
		message: &poise::serenity_prelude::Message,
	) -> Result<(&'a str, usize, Self), (PoiseError, Option<String>)> {
		if let Some(code_block_start) = args.find("```") {
			args = &args[code_block_start..];
		}

		let (rest, attachment_index, code_block) =
			poise::prefix_argument::CodeBlock::pop_from(args, attachment_index, ctx, message).await?;

		let mut source = code_block.code;
		// Strip ANSI escapes if provided.
		if code_block.language.as_deref() == Some("ansi") {
			source = strip_ansi_escapes::strip_str(source);
		}

		// Remove all occurrences of zero width joiners surrounded by backticks.
		// This is used to enter Typst code blocks within Discord-markdown code blocks.
		// Two replace calls are needed to remove all patterns of `ABA`: ABABABA => AABAA => AAAA.
		let pattern = format!("`{ZERO_WIDTH_JOINER}`");
		let replacement = "``";
		source = source
			.replace(&pattern, replacement)
			.replace(&pattern, replacement);

		Ok((rest, attachment_index, CodeBlock { source }))
	}
}

struct Rest;

#[async_trait]
impl<'a> poise::PopArgument<'a> for Rest {
	async fn pop_from(
		_args: &'a str,
		attachment_index: usize,
		_ctx: &serenity::prelude::Context,
		_message: &poise::serenity_prelude::Message,
	) -> Result<(&'a str, usize, Self), (PoiseError, Option<String>)> {
		Ok(("", attachment_index, Self))
	}
}

#[poise::command(
	prefix_command,
	track_edits,
	broadcast_typing,
	user_cooldown = 1,
	help_text_fn = "render_help",
	aliases("r")
)]
async fn render(
	ctx: Context<'_>,
	flags: RenderFlags,
	code: CodeBlock,
	#[rename = "rest"] _: Rest,
) -> Result<(), PoiseError> {
	let pool = &ctx.data().pool;

	let mut source = code.source;
	source.insert_str(0, &flags.preamble.preamble());

	let mut progress = String::new();
	let (progress_send, mut progress_recv) = mpsc::channel(4);
	let (res, ()) = {
		let mut pool = pool.lock().await;
		join!(pool.render(source, progress_send), async {
			// When `render` finishes, it will drop the sender so this loop will finish.
			while let Some(item) = progress_recv.recv().await {
				progress.reserve(item.len() + 1);
				progress.push_str(&item);
				progress.push('\n');
				let message = format!("Progress: ```ansi\n{}\n```", sanitize_code_block(&progress));
				_ = ctx.say(message).await;
			}
		})
	};

	match res {
		Ok(res) => {
			let mut message = CreateReply::default().reply(true);

			let mut content = String::new();

			if res.images.is_empty() {
				writeln!(content, "Note: no pages generated").unwrap();
			}

			if res.more_pages > 0 {
				let more_pages = res.more_pages;
				writeln!(
					content,
					"Note: {more_pages} more page{s} ignored",
					s = if more_pages == 1 { "" } else { "s" },
				)
				.unwrap();
			}

			if !res.warnings.is_empty() {
				writeln!(
					content,
					"Render succeeded with warnings:\n```ansi\n{}\n```",
					sanitize_code_block(&res.warnings),
				)
				.unwrap();
			}

			if !content.is_empty() {
				message = message.content(content);
			}

			for (i, image) in res.images.into_iter().enumerate() {
				let image = CreateAttachment::bytes(image, format!("page-{}.png", i + 1));
				message = message.attachment(image);
			}

			ctx.send(message).await?;
		}
		Err(error) => {
			let message = format!(
				"An error occurred:\n```ansi\n{}\n```",
				sanitize_code_block(&format!("{error:?}")),
			);
			ctx.reply(message).await?;
		}
	}

	Ok(())
}

/// Show this menu.
#[poise::command(prefix_command, track_edits, slash_command)]
async fn help(
	ctx: Context<'_>,
	#[description = "Specific command to show help about"] command: Option<String>,
) -> Result<(), PoiseError> {
	// Avoid the useless parameter list from `poise::builtins::help`.
	if command.as_deref() == Some("render") {
		// We prefer `reply` but `poise::builtins::help` uses `say`, so be consistent.
		ctx.say(format!("`?render`\n\n{}", render_help())).await?;
		return Ok(());
	}

	let config = poise::builtins::HelpConfiguration {
		extra_text_at_bottom: "\
Type ?help command for more info on a command.
You can edit your message to the bot and the bot will edit its response.
Commands prefixed with / can be used as slash commands and prefix commands.
Commands prefixed with ? can only be used as prefix commands.
The bot is written by mattf_. Feel free to reach out in the Typst Discord if you have any questions.
",
		..Default::default()
	};
	poise::builtins::help(ctx, command.as_deref(), config).await?;
	Ok(())
}

/// Get a link to the bot's source.
#[poise::command(prefix_command, slash_command)]
async fn source(ctx: Context<'_>) -> Result<(), PoiseError> {
	ctx.reply(format!("<{SOURCE_URL}>")).await?;
	Ok(())
}

/// Get the AST for the given code.
///
/// Syntax: `?ast <code block> [...]`
///
/// **Examples**
///
/// ```
/// ?ast `hello, world!`
///
/// ?ast ``‍`
/// = Heading!
///
/// And some text.
///
/// #lorem(100)
/// ``‍`
///
/// ?ast `#((3): 4)` Interesting parse result here.
/// ```
#[poise::command(prefix_command, track_edits, broadcast_typing)]
async fn ast(
	ctx: Context<'_>,
	#[description = "Code to parse"] code: CodeBlock,
	#[rename = "rest"]
	#[description = "Extra message content"]
	_: Rest,
) -> Result<(), PoiseError> {
	let pool = &ctx.data().pool;

	let res = pool.lock().await.ast(code.source).await;

	match res {
		Ok(ast) => {
			let message = format!("```ansi\n{}```", sanitize_code_block(&ast));
			ctx.reply(message).await?;
		}
		Err(error) => {
			let message = format!(
				"An error occurred:\n```ansi\n{}```",
				sanitize_code_block(&format!("{error:?}")),
			);
			ctx.reply(message).await?;
		}
	}

	Ok(())
}

/// Show the bot's Typst version.
#[poise::command(prefix_command, slash_command)]
async fn version(ctx: Context<'_>) -> Result<(), PoiseError> {
	let pool = &ctx.data().pool;

	let res = pool.lock().await.version().await;

	match res {
		Ok(VersionResponse {
			version: typst_version,
		}) => {
			let bot_hash = env!("BUILD_SHA");
			let message = format!("\
The bot was built from git hash [`{bot_hash}`](<https://github.com/mattfbacon/typst-bot/tree/{bot_hash}>)
The bot is using Typst [version **{typst_version}**](<https://github.com/typst/typst/releases/v{typst_version}>)\
");
			ctx.reply(message).await?;
		}
		Err(error) => {
			let message = format!("An error occurred:\n```ansi\n{error}```");
			ctx.reply(message).await?;
		}
	}

	Ok(())
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

impl From<TagName> for String {
	fn from(value: TagName) -> Self {
		value.0
	}
}

/// Performs autocomplete of tags, through a "fuzzy" search (matches all tags containing the partial string).
/// Must be an async function for poise to accept it as a valid autocomplete function.
/// Can only return up to 25 tags due to a Discord limitation.
#[allow(clippy::unused_async)]
async fn tag_autocomplete(ctx: Context<'_>, partial_tag: &str) -> Vec<TagName> {
	let database = &ctx.data().database;
	let Ok(database) = database.lock() else {
		return Vec::new();
	};

	let Some(guild_id) = ctx.guild_id() else {
		return Vec::new();
	};

	database
		.prepare("select name from tags where INSTR(name, :name) and guild = :guild limit 25")
		.and_then(|mut statement|
			// Convert `Vec<Result<String>>` into `Result<Vec<TagName>>` (abort if one of the rows failed).
			statement
			.query_and_then(
				named_params!(":name": partial_tag, ":guild": guild_id.get()),
				|row| row.get::<_, String>("name")
			)
			.and_then(|rows| rows.map(|row| row.map(TagName)).collect::<Result<Vec<_>, _>>()))
		.unwrap_or_else(|_| Vec::new())
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
	// Remove last `%`.
	buf.pop();
	buf
}

/// Print the content of a tag by name.
///
/// Syntax: `?tag <tag name> <parameters...>`
///
/// If the tag has placeholders (set with `%s`),
/// then you can fill them with the subsequent arguments.
///
/// Note that tags are local to the guild.
#[poise::command(prefix_command, slash_command, track_edits)]
async fn tag(
	ctx: Context<'_>,
	#[rename = "tag_name"]
	#[description = "The tag to print"]
	#[autocomplete = "tag_autocomplete"]
	TagName(tag_name): TagName,
	#[description = "Any parameters for the tag"] parameters: Vec<String>,
) -> Result<(), PoiseError> {
	let database = &ctx.data().database;
	let guild_id = ctx.guild_id().ok_or("no guild id, so no tags")?.get();
	let text = database
		.lock()
		.map_err(|_| "db mutex poisoned, oops")?
		.prepare("select text from tags where name = :name and guild = :guild")?
		.query(named_params!(":name": tag_name, ":guild": guild_id))?
		.next()?
		.map(|row| row.get::<_, String>("text"))
		.transpose()?;
	let text = text.unwrap_or_else(|| "That tag is not defined.".into());
	let text = interpolate(&text, parameters.iter().map(String::as_str));
	ctx.say(text).await?;
	Ok(())
}

/// Set the content of a tag (privileged).
///
/// Syntax: `?set-tag <tag name> <tag text>`
///
/// Note that tags are local to the guild.
#[poise::command(
	prefix_command,
	slash_command,
	rename = "set-tag",
	invoke_on_edit,
	required_permissions = "KICK_MEMBERS"
)]
async fn set_tag(
	ctx: Context<'_>,
	#[rename = "tag_name"]
	#[description = "The tag to define"]
	TagName(tag_name): TagName,
	#[rest]
	#[rename = "tag_text"]
	#[description = "The text of the tag"]
	#[max_length = 1000]
	tag_text: String,
) -> Result<(), PoiseError> {
	let database = &ctx.data().database;

	let guild_id = ctx.guild_id().ok_or("no guild id, so no tags")?.get();
	database.lock()
		.map_err(|_| "db mutex poisoned, oops")?
		.execute(
		"insert into tags (name, guild, text) values (:name, :guild, :text) on conflict do update set text = :text",
		named_params!(":name": tag_name, ":guild": guild_id, ":text": tag_text),
	)?;

	let author = ctx.author().id;
	let message = format!("Tag {tag_name:?} updated by <@{author}>: {tag_text}");
	let message = CreateReply::default()
		.content(message)
		.reply(true)
		.ephemeral(true);
	ctx.send(message).await?;

	Ok(())
}

/// Delete a tag (privileged).
///
/// Syntax: `?delete-tag <tag name>`
#[poise::command(
	prefix_command,
	slash_command,
	rename = "delete-tag",
 // It doesn't undo deletion, so it's not exactly a purely edit-tracked system, but users still expect this type of behavior.
	invoke_on_edit,
	required_permissions = "KICK_MEMBERS"
)]
async fn delete_tag(
	ctx: Context<'_>,
	#[rename = "tag_name"]
	#[description = "The tag to delete"]
	TagName(tag_name): TagName,
) -> Result<(), PoiseError> {
	let database = &ctx.data().database;

	let guild_id = ctx.guild_id().ok_or("no guild id, so no tags")?.get();
	let num_rows = database
		.lock()
		.map_err(|_| "db mutex poisoned, oops")?
		.execute(
			"delete from tags where name = :name and guild = :guild",
			named_params!(":name": tag_name, ":guild": guild_id),
		)?;

	let message = if num_rows > 0 {
		format!("Tag {tag_name:?} deleted by <@{}>", ctx.author().id)
	} else {
		format!("Tag {tag_name:?} not found")
	};

	let message = CreateReply::default()
		.content(message)
		.reply(true)
		.ephemeral(true);
	ctx.send(message).await?;

	Ok(())
}

/// List all tags.
///
/// Syntax: `?tags [filter]`
///
/// If `filter` is included, it will only show tags whose names include the given text.
#[poise::command(prefix_command, slash_command, rename = "tags", track_edits)]
async fn list_tags(
	ctx: Context<'_>,
	#[rename = "filter"]
	#[description = "Show tags with this in their name"]
	#[max_length = 20]
	filter: Option<String>,
) -> Result<(), PoiseError> {
	let reply = {
		let database = &ctx.data().database;
		let database = database.lock().map_err(|_| "db mutex poisoned, oops")?;
		let mut statement = database.prepare(
			"select name from tags where guild = :guild and (:filter is null or instr(name, :filter) > 0) order by name",
		)?;
		let guild_id = ctx.guild_id().ok_or("no guild id, so no tags")?.get();
		let mut results = statement.query_map(
			named_params!(":filter": filter, ":guild": guild_id),
			|row| row.get::<_, Box<str>>("name"),
		)?;
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

	ctx.reply(reply).await?;

	Ok(())
}

async fn handle_error(
	error: poise::FrameworkError<'_, Data, Box<dyn std::error::Error + Send + Sync>>,
) -> serenity::Result<()> {
	if let poise::FrameworkError::ArgumentParse {
		ctx, input, error, ..
	} = error
	{
		let name = &ctx.command().name;
		let usage = format!(
			"Use `?help {name}` for usage. Feel free to edit or delete your message and the bot will react.",
		);
		let response = input.map_or_else(
			|| format!("**{error}**\n{usage}"),
			|input| format!("**Cannot parse `{input}` as argument: {error}**\n{usage}"),
		);
		ctx.reply(response).await?;
		Ok(())
	} else {
		poise::builtins::on_error(error).await
	}
}

pub async fn run() {
	let database = Connection::open_with_flags(
		std::env::var_os("DB_PATH").expect("need `DB_PATH` env var"),
		OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
	)
	.unwrap();
	database.execute("create table if not exists tags (name text not null, guild integer not null, text text not null, unique (name, guild)) strict", []).unwrap();
	let database = std::sync::Mutex::new(database);

	let pool = Worker::spawn().await.unwrap();

	let edit_tracker_time = std::time::Duration::from_secs(3600);

	let token = std::env::var("DISCORD_TOKEN").expect("need `DISCORD_TOKEN` env var");
	let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;
	let framework = poise::Framework::builder()
		.options(poise::FrameworkOptions {
			prefix_options: poise::PrefixFrameworkOptions {
				prefix: Some("?".to_owned()),
				edit_tracker: Some(poise::EditTracker::for_timespan(edit_tracker_time).into()),
				..Default::default()
			},
			commands: vec![
				render(),
				help(),
				source(),
				ast(),
				version(),
				tag(),
				set_tag(),
				delete_tag(),
				list_tags(),
			],
			allowed_mentions: Some(CreateAllowedMentions::new()),
			on_error: |error| {
				Box::pin(async move {
					if let Err(error) = handle_error(error).await {
						tracing::error!(?error, "Error while handling error");
					}
				})
			},
			..Default::default()
		})
		.setup(|ctx, _ready, framework| {
			Box::pin(async move {
				poise::builtins::register_globally(ctx, &framework.options().commands).await?;
				Ok(Data {
					pool: Mutex::new(pool),
					database,
				})
			})
		})
		.build();

	let mut client = serenity::client::ClientBuilder::new(token, intents)
		.framework(framework)
		.await
		.unwrap();

	eprintln!("ready");

	client.start().await.unwrap();
}
