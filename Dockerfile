FROM rust:1.72


# install git lfs
RUN curl -s https://packagecloud.io/install/repositories/github/git-lfs/script.deb.sh | bash
RUN apt-get install git-lfs


# download repository
RUN git lfs clone https://github.com/mattfbacon/typst-bot


# compile
WORKDIR /typst-bot/worker
RUN cargo build --release

WORKDIR /typst-bot/bot
RUN cargo build --release


# cleanup
RUN mkdir /bot
RUN mkdir /bot/cache
RUN cp /typst-bot/target/release/worker /bot/worker
RUN cp /typst-bot/target/release/typst-bot /bot/typst-bot
RUN cp -r /typst-bot/fonts /bot/fonts

WORKDIR /bot

RUN rm -r /typst-bot


# run
CMD [ "/bot/typst-bot" ]
