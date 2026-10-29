# Sandbox for third-party MCP servers in the real-world benchmark: one container per server,
# no host directories mounted, packages fetched by npx / uvx into a shared cache volume.
# Build: docker build -t toollens-sandbox:1 -f bench/real/sandbox.Dockerfile bench/real
FROM node:22-bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates git \
 && rm -rf /var/lib/apt/lists/*

COPY --from=ghcr.io/astral-sh/uv:0.8.17 /uv /uvx /usr/local/bin/

# Package caches live on a named volume so each server is downloaded once, not per container.
ENV npm_config_cache=/cache/npm \
    npm_config_update_notifier=false \
    NO_UPDATE_NOTIFIER=1 \
    UV_CACHE_DIR=/cache/uv \
    UV_PYTHON_INSTALL_DIR=/cache/python \
    UV_TOOL_DIR=/cache/uv-tools

# AWS's documented example credentials (they authenticate nothing): AWS Labs servers refuse to
# start without a default profile, and calls then fail authentication like every other dummy key.
RUN mkdir -p /root/.aws \
 && printf '[default]\nregion = us-east-1\n' > /root/.aws/config \
 && printf '[default]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n' > /root/.aws/credentials

# A small workspace and git repository for the filesystem and git servers' tasks.
RUN mkdir -p /workspace/repo /workspace/notes \
 && printf 'Quarterly targets: revenue 4.2M, churn under 3%%.\n' > /workspace/notes/q3-plan.md \
 && printf 'name,owner\nbilling,priya\nsearch,jo\n' > /workspace/notes/services.csv \
 && cd /workspace/repo \
 && git init -q -b main \
 && git -c user.name=Ana -c user.email=ana@example.com commit -q --allow-empty -m "Initial commit" \
 && printf 'retry_limit = 5\n' > config.toml \
 && git add config.toml \
 && git -c user.name=Ana -c user.email=ana@example.com commit -q -m "Raise retry limit to 5 for payment webhooks"
WORKDIR /workspace
