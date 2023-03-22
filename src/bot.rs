use std::collections::HashMap;
use std::sync::Arc;

use poise::async_trait;
use poise::serenity_prelude::{AttachmentType, GatewayIntents};
use typst::geom::{Color, RgbaColor};

use crate::sandbox::Sandbox;
use crate::SOURCE_URL;

struct Data {
	sandbox: Arc<Sandbox>,
}

type PoiseError = Box<dyn std::error::Error + Send + Sync + 'static>;
type Context<'a> = poise::Context<'a, Data, PoiseError>;

#[derive(Debug)]
struct RenderFlags {
	fill: Color,
}

impl Default for RenderFlags {
	fn default() -> Self {
		Self { fill: Color::WHITE }
	}
}

fn parse_color(raw: &str) -> Result<Color, impl std::error::Error> {
	csscolorparser::parse(raw).map(|css| {
		let [r, g, b, a] = css.to_rgba8();
		RgbaColor { r, g, b, a }.into()
	})
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
					"fill" => {
						parsed.fill = parse_color(value).map_err(|error| format!("invalid fill: {error}"))?;
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

/// Render the given code as an image.
///
/// Render the given code as an image.
///
/// Syntax: `?render [fill=<color>] <code block>`
///
/// **Examples**
///
/// ```
/// ?render `hello, world!`
///
/// ?render fill=black ``‌`
/// 	#set text(color: white)
/// 	= Heading!
/// 	And some text.
/// ``‌`
/// ```
#[poise::command(prefix_command, track_edits, broadcast_typing, user_cooldown = 1)]
async fn render(
	ctx: Context<'_>,
	#[description = "Flags"] flags: RenderFlags,
	#[description = "Code to render"] code: poise::prefix_argument::CodeBlock,
) -> Result<(), PoiseError> {
	let sandbox = Arc::clone(&ctx.data().sandbox);

	let res =
		tokio::task::spawn_blocking(move || crate::render::render(sandbox, flags.fill, code.code))
			.await?;

	match res {
		Ok(image) => {
			ctx
				.send(|reply| {
					reply
						.attachment(AttachmentType::Bytes {
							data: image.into(),
							filename: "rendered.png".into(),
						})
						.reply(true)
				})
				.await?;
		}
		Err(error) => {
			ctx
				.send(|reply| {
					// U+200C is a zero-width non-joiner. It prevents the triple backtick from being interpreted as a codeblock but retains ligature support.
					let error = error.to_string().replace("```", "``\u{200c}`");
					reply
						.content(format!("An error occurred:\n```typst\n{error}```"))
						.reply(true)
				})
				.await?;
		}
	}

	Ok(())
}

/// Show this menu
#[poise::command(prefix_command, track_edits, slash_command)]
async fn help(
	ctx: Context<'_>,
	#[description = "Specific command to show help about"] command: Option<String>,
) -> Result<(), PoiseError> {
	let config = poise::builtins::HelpConfiguration {
		extra_text_at_bottom: "\
Type ?help command for more info on a command.
You can edit your message to the bot and the bot will edit its response.
Commands prefixed with / can be used as slash commands and prefix commands.
Commands prefixed with ? can only be used as prefix commands.",
		..Default::default()
	};
	poise::builtins::help(ctx, command.as_deref(), config).await?;
	Ok(())
}

/// Get a link to the bot's source.
#[poise::command(prefix_command, slash_command)]
async fn source(ctx: Context<'_>) -> Result<(), PoiseError> {
	ctx
		.send(|reply| reply.content(format!("<{SOURCE_URL}>")).reply(true))
		.await?;

	Ok(())
}

pub async fn run() {
	let sandbox = Arc::new(Sandbox::new());

	eprintln!("ready");

	let edit_tracker_time = std::time::Duration::from_secs(3600);

	let framework = poise::Framework::builder()
		.options(poise::FrameworkOptions {
			prefix_options: poise::PrefixFrameworkOptions {
				prefix: Some("?".to_owned()),
				edit_tracker: Some(poise::EditTracker::for_timespan(edit_tracker_time)),
				..Default::default()
			},
			commands: vec![render(), help(), source()],
			..Default::default()
		})
		.token(std::env::var("DISCORD_TOKEN").expect("need `DISCORD_TOKEN` env var"))
		.intents(GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT)
		.setup(|ctx, _ready, framework| {
			Box::pin(async move {
				poise::builtins::register_globally(ctx, &framework.options().commands).await?;
				Ok(Data { sandbox })
			})
		});

	framework.run().await.unwrap();
}
