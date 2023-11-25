# ====== Build Stage ======
FROM rust:1.72-bullseye AS build

# Copy all source code into image and compile.
COPY . /typst-bot

WORKDIR /typst-bot
RUN cargo build --release --all --config git-fetch-with-cli=true


# ====== Run stage ======
FROM debian:bullseye-slim

# The only files we need to run the bot are the two executables, the fonts, and a database.
RUN mkdir -p /bot/cache/ /bot/fonts/ \
    && touch /bot/db.sqlite

COPY --from=build \
    /typst-bot/target/release/worker \
    /typst-bot/target/release/typst-bot \
    /bot/
COPY --from=build /typst-bot/fonts/ \
    /bot/fonts/

# These variables can get burned into the image without issue. `DISCORD_TOKEN` needs to come from
# the user or .env, though.
ENV DB_PATH=/bot/db.sqlite \
    CACHE_DIRECTORY=/bot/cache

WORKDIR /bot
CMD [ "/bot/typst-bot" ]
