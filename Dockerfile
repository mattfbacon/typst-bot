# Build Stage
FROM rust:1.72 AS builder

RUN curl -s https://packagecloud.io/install/repositories/github/git-lfs/script.deb.sh | bash
RUN apt-get install git-lfs

RUN git clone https://github.com/mattfbacon/typst-bot

WORKDIR /typst-bot
RUN cargo build --release --all


# Run Stage
FROM debian as prod

RUN mkdir /bot

RUN mkdir /bot/cache
COPY --from=builder /typst-bot/target/release/worker /bot/worker
COPY --from=builder /typst-bot/target/release/typst-bot /bot/typst-bot
COPY --from=builder /typst-bot/fonts /bot/fonts

WORKDIR /bot
ENV CACHE_DIRECTORY=cache
CMD [ "/bot/typst-bot" ]
