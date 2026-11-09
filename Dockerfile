# Download Craft in server mode: no window; the queue runs by itself and the
# remote control page is served on port 8765 for phones and browsers.
#
#   docker build -t downloadcraft .
#   docker run -d --name downloadcraft -p 8765:8765 -p 6881:6881 \
#     -v dc-data:/data -v /path/to/downloads:/downloads downloadcraft
#   docker logs downloadcraft          # shows the address and pairing code
#   docker exec downloadcraft download_craft --server-code   # a new code

FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
      pkg-config libx11-dev libxi-dev libxcursor-dev libxrandr-dev libgl1-mesa-dev \
      libdbus-1-dev libxkbcommon-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked --bin download_craft \
    && strip target/release/download_craft

FROM debian:bookworm-slim
# The program is one binary with the window built in; server mode never opens
# it, but the libraries it links against must be present.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates libx11-6 libxi6 libxcursor1 libxrandr2 libgl1 libdbus-1-3 libxkbcommon0 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 1000 craft \
    && mkdir -p /data /downloads && chown craft:craft /data /downloads
COPY --from=build /src/target/release/download_craft /usr/local/bin/download_craft
USER craft
ENV DOWNLOAD_CRAFT_DATA=/data \
    DOWNLOAD_CRAFT_DOWNLOADS=/downloads \
    DOWNLOAD_CRAFT_PORT=8765 \
    DOWNLOAD_CRAFT_TORRENT_PORT=6881
VOLUME ["/data", "/downloads"]
EXPOSE 8765 6881
ENTRYPOINT ["download_craft", "--server"]
