# The gateway on stdio. With no arguments it runs the built-in demo servers
# (examples/gateway.json), which is what MCP directories start to list its tools. For your own
# servers mount a directory holding gateway.json and run
# `serve --config /config/gateway.json --db /app/toollens.db` (README, "In Docker"): npx and uvx
# servers run inside this container.
FROM rust:1.89-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo build --release --locked

FROM node:22-bookworm-slim
LABEL io.modelcontextprotocol.server.name="io.github.krishnashakula/toollens"
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=ghcr.io/astral-sh/uv:0.8.17 /uv /uvx /usr/local/bin/
ENV npm_config_update_notifier=false NO_UPDATE_NOTIFIER=1
COPY --from=build /src/target/release/toollens /usr/local/bin/toollens
COPY examples/*.json /app/examples/
WORKDIR /app
ENTRYPOINT ["toollens"]
CMD ["serve", "--config", "examples/gateway.json"]
