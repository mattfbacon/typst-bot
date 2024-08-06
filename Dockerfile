# ============ Build Stage ============
FROM rust:1.74-bookworm as build

WORKDIR /typst-bot

# Compilation requires only the source code.
COPY Cargo.toml Cargo.lock ./
COPY crates crates

RUN cargo build --release --all --config git-fetch-with-cli=true


# ============ Run Stage ============
FROM debian:bookworm-slim as run

WORKDIR /bot
CMD [ "/bot/typst-bot" ]

# These variables can get burned into the image without issue. We don't want `DISCORD_TOKEN` saved
# in the image, though; it needs to come from the user (or from Compose) when the container is run.
ENV DB_PATH=/bot/sqlite/db.sqlite \
    CACHE_DIRECTORY=/bot/cache

# Create the necessary directories and the empty database file
RUN mkdir -p /bot/sqlite /bot/cache && \
    touch /bot/sqlite/db.sqlite

# The only files we need from the build stage in order to run the bot are the two executables.
COPY --from=build \
    /typst-bot/target/release/worker \
    /typst-bot/target/release/typst-bot \
    ./

# Fonts are copied from the host at the very end so that the fonts can get updated without
# invalidating any previously cached image layers.
COPY fonts fonts
