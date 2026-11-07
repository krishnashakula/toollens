# The gateway over its built-in demo servers (examples/gateway.json), on stdio: what MCP
# directories run to check a server and list its tools. For your own servers use a release
# binary; the servers in your config would otherwise have to be installed in this image.
FROM rust:1.89-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo build --release --locked

FROM debian:bookworm-slim
LABEL io.modelcontextprotocol.server.name="io.github.krishnashakula/toollens"
COPY --from=build /src/target/release/toollens /usr/local/bin/toollens
COPY examples/*.json /app/examples/
WORKDIR /app
ENTRYPOINT ["toollens"]
CMD ["serve", "--config", "examples/gateway.json"]
