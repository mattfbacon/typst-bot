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

fn expressions(content: &str) -> impl Iterator<Item = &str> {
	content
		.split("$$")
		.skip(1)
		.step_by(2)
}

#[serenity::async_trait]
impl EventHandler for InlineMathHandler {
	async fn message(&self, ctx: Context, message: Message) {
		if message.author.bot {
			return;
		}

		for expression in expressions(&message.content).filter(|expression| !expression.trim().is_empty()) {
			let mut source = String::with_capacity(PREAMBLE.len() + expression.len());
			source.push_str(PREAMBLE);
			source.push_str(expression);

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
				Ok(_) => {}
				Err(error) => tracing::warn!(?error, "Failed to render inline math"),
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
	use super::expressions;

	#[test]
	fn extracts_all_dollar_delimited_expressions() {
		assert_eq!(
			expressions("Test $$1+1=3$$ and $$2+3=5$$").collect::<Vec<_>>(),
			["1+1=3", "2+3=5"],
		);
	}

	#[test]
	fn ignores_unclosed_delimiters() {
		assert_eq!(expressions("$$1+1$$ and $$unfinished").collect::<Vec<_>>(), ["1+1"]);
	}
}
