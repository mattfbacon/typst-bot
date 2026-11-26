use poise::serenity_prelude::{GatewayIntents, Message};
use serenity::all::{Context, CreateAttachment, CreateMessage, EventHandler};
use tokio::sync::{mpsc, Mutex};

use crate::worker::Worker;

const PREAMBLE: &str = concat!(
	"// Begin preamble\n",
	"// Page size:\n",
	"#set page(width: 300pt, height: auto, margin: 10pt)\n",
	"// Theme:\n",
	"#set page(fill: rgb(49, 51, 56))\n",
	"#set text(fill: rgb(219, 222, 225))\n",
	"// End preamble\n",
);

struct InlineMathHandler {
	worker: Mutex<Worker>,
}

fn source_for_message(content: &str) -> Option<String> {
	content.contains('$').then(|| format!("{PREAMBLE}{content}"))
}

#[serenity::async_trait]
impl EventHandler for InlineMathHandler {
	async fn message(&self, ctx: Context, message: Message) {
		if message.author.bot {
			return;
		}

		if let Some(source) = source_for_message(&message.content) {
			let (progress_sender, _progress_receiver) = mpsc::channel(1);
			let rendered = self.worker.lock().await.render(source, progress_sender).await;
			match rendered {
				Ok(rendered) if !rendered.images.is_empty() => {
					let mut reply = CreateMessage::new().reference_message(&message);
					for (index, image) in rendered.images.into_iter().enumerate() {
						reply = reply.add_file(CreateAttachment::bytes(image, format!("page-{}.png", index + 1)));
					}
					if let Err(error) = message.channel_id.send_message(&ctx.http, reply).await {
						tracing::error!(?error, "Failed to send inline math render");
					}
				}
				Ok(_) => {
					let reply = CreateMessage::new()
						.reference_message(&message)
						.content("The message rendered without producing any pages.");
					if let Err(error) = message.channel_id.send_message(&ctx.http, reply).await {
						tracing::error!(?error, "Failed to send empty inline math render response");
					}
				}
				Err(error) => {
					tracing::warn!(?error, "Failed to render inline math");
					let reply = CreateMessage::new()
						.reference_message(&message)
						.content(format!("Failed to render message: {error}"));
					if let Err(error) = message.channel_id.send_message(&ctx.http, reply).await {
						tracing::error!(?error, "Failed to send inline math render error");
					}
				}
			}
		}
	}
}

pub async fn run() {
	let token = match (std::env::var_os("DISCORD_TOKEN"), std::env::var_os("DISCORD_TOKEN_FILE")) {
		(Some(token), None) => token.into_string().expect("`DISCORD_TOKEN` not UTF-8"),
		(None, Some(path)) => std::fs::read_to_string(path).expect("reading from `DISCORD_TOKEN_FILE`"),
		(Some(_), Some(_)) => panic!("both `DISCORD_TOKEN` and `DISCORD_TOKEN_FILE` provided"),
		(None, None) => panic!("need `DISCORD_TOKEN` or `DISCORD_TOKEN_FILE` env var"),
	};

	let handler = InlineMathHandler {
		worker: Mutex::new(Worker::spawn().await.expect("spawning Typst worker")),
	};
	let intents = GatewayIntents::GUILD_MESSAGES
		| GatewayIntents::DIRECT_MESSAGES
		| GatewayIntents::MESSAGE_CONTENT;
	let mut client = serenity::Client::builder(token.trim(), intents)
		.event_handler(handler)
		.await
		.expect("creating inline math Discord client");

	if let Err(error) = client.start().await {
		tracing::error!(?error, "Inline math Discord client stopped");
	}
}

#[cfg(test)]
mod tests {
	use super::{source_for_message, PREAMBLE};

	#[test]
	fn renders_the_entire_message_when_it_contains_a_dollar_sign() {
		let message = "Text before $x$ and after.";
		assert_eq!(
			source_for_message(message).as_deref(),
			Some(format!("{PREAMBLE}{message}").as_str()),
		);
	}

	#[test]
	fn does_not_render_messages_without_a_dollar_sign() {
		assert_eq!(source_for_message("No math here"), None);
	}

	#[test]
	fn renders_messages_with_single_or_unpaired_dollar_signs() {
		assert_eq!(source_for_message("Price: $5").as_deref(), Some(format!("{PREAMBLE}Price: $5").as_str()));
	}
}
