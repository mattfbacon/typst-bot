# Typst Bot

A Discord bot that renders Typst code.

Built with poise so it has all the goodies like edit tracking, typing status, and automatic help generation.

## Hosting

To set up, create a directory with the following items:

- `fonts`: Copied from the repo. Make sure you have Git LFS set up so the fonts are downloaded properly.
- `worker`: the worker binary, copied from the target directory after building.

To run, enter the bot's working directory, set `DISCORD_TOKEN` to your bot token, and run the bot binary.

## License

AGPL. Use `?source` to get a link to the source from deployments of the bot.
