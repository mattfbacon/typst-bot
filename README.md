# Typst Slack Bot

A Slack bot that renders Typst code when mentioned.

Use it in Slack as:

```text
@typst-bot render `hello, world!`
```

The bot listens for Slack Events API `app_mention` events. Slack sends mentions as
`<@BOT_USER_ID> render ...`; users type the friendlier `@typst-bot render ...`.

## Hosting

The bot uses two binaries:

- `typst-bot`: receives Slack HTTP events and calls Slack Web API methods
- `worker`: receives render requests, interacts with Typst, responds over IPC

`typst-bot` automatically spawns `worker`, so you only need to run `typst-bot`.

To set up the working environment, create a directory with the following items:

- `worker`: The worker binary. If you put the worker somewhere else, set
  `TYPST_BOT_WORKER_PATH`.
- `typst-bot`: The bot binary.
- `db.sqlite`: You can just `touch` this, but the bot needs to be able to write to it.

Required environment variables:

- `SLACK_BOT_TOKEN` or `SLACK_BOT_TOKEN_FILE`
- `SLACK_SIGNING_SECRET` or `SLACK_SIGNING_SECRET_FILE`
- `DB_PATH`
- `CACHE_DIRECTORY`

Optional environment variables:

- `BIND_ADDR`, default `0.0.0.0:3000`
- `TYPST_BOT_WORKER_PATH`, default `./worker`
- `SLACK_TAG_ADMIN_USERS`, comma-separated Slack user IDs allowed to change tags.
  If unset, anyone can change channel-local tags.

In your Slack app, enable Event Subscriptions and set the Request URL to:

```text
https://your-public-host.example/slack/events
```

Subscribe the bot to the `app_mention` bot event. Required bot token scopes:

- `app_mentions:read`
- `chat:write`
- `files:write`

The bot replies in a thread under the mention. Rendered PNGs are uploaded using
Slack's current external upload flow, not the retired `files.upload` method.

### Docker

There is a `Dockerfile` and `docker-compose.yml` for running the bot inside a
Docker container.

Create these files:

- `slack_bot_token.txt`: your bot token
- `slack_signing_secret.txt`: your Slack signing secret

Then start the container:

```sh
docker compose up -d
```

The container listens on port `3000`. Put it behind HTTPS before connecting it
to Slack Events API.

### Commands

- `@typst-bot render` / `@typst-bot r`: render Typst code
- `@typst-bot ast`: show Typst AST
- `@typst-bot version`: show Typst version
- `@typst-bot source`: show source URL
- `@typst-bot tag`, `set-tag`, `delete-tag`, `tags`: channel-local tags

Run `@typst-bot help render` for render flags and examples.

### Is it safe to host? Is it true that Typst allows arbitrary code execution?

Typst is fundamentally a sandboxed, interpreted language so there is no such
thing as "arbitrary code execution". However, Typst documents/code can access
the host environment in a limited capacity. In CLI usage, documents can read
files inside the project directory and download packages from the Typst package
repo. For the bot, only the latter is allowed. Resource exhaustion and DOS
attacks are also addressed with timeouts and automatic worker restarting.

For public deployments, use HTTPS, keep `SLACK_SIGNING_SECRET` private, and put
the process behind normal host-level sandboxing such as Docker or systemd.

## License

AGPL. Use `@typst-bot source` to get a link to the source from deployments of
the bot.
