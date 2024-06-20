FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /typst-bot

FROM chef AS planner

# Compilation requires only the source code.
COPY Cargo.toml Cargo.lock ./
COPY protocol protocol
COPY worker worker
COPY bot bot
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /typst-bot/recipe.json recipe.json
# Make sure the flags given are the same as in the `cargo build` line later on, so that 
RUN cargo chef cook --release --workspace --recipe-path recipe.json
# Compilation requires only the source code.
COPY Cargo.toml Cargo.lock ./
COPY protocol protocol
COPY worker worker
COPY bot bot
RUN cargo build --release --workspace --config git-fetch-with-cli=true

# ============ Run Stage ============
FROM debian:bookworm-slim as run

WORKDIR /bot
CMD [ "/bot/typst-bot" ]

# These variables can get burned into the image without issue. We don't want `DISCORD_TOKEN` saved
# in the image, though; it needs to come from the user (or from Compose) when the container is run.
ENV DB_PATH=/bot/sqlite/db.sqlite \
    CACHE_DIRECTORY=/bot/cache

# The only files we need from the build stage in order to run the bot are the two executables.
COPY --from=builder \
    /typst-bot/target/release/worker \
    /typst-bot/target/release/typst-bot \
    ./

# Fonts are copied from the host at the very end so that the fonts can get updated without
# invalidating any previously cached image layers.
COPY fonts fonts

