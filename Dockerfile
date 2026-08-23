# syntax=docker/dockerfile:1
# ============================================================================
# MCPG Inspector TUI — release mirror image
# ----------------------------------------------------------------------------
# A containerized interactive terminal client. Run it with a TTY:
#
#   docker run -it --rm ghcr.io/mcpg-dev/mcpg-inspector-tui <args>
#
# Self-contained build from this repository alone; the TUI draws over raw
# ANSI (no terminfo dependency), so the runtime carries only certificates
# and an init. Sibling crates are consumed by git reference; when one is
# private the Rust stage takes a fetch token as a BuildKit secret:
#
#   docker build --secret id=sibling_fetch_token,env=SIBLING_FETCH_TOKEN -t inspector-tui:local .
#
# Without the secret the build still works when every referenced sibling
# is public — fetches just stay anonymous.
# ============================================================================

FROM rust:1-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        cmake clang libclang-dev perl pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# Private sibling git fetches authenticate through the BuildKit secret for
# exactly one RUN; the credential file is removed in the same layer.
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
RUN --mount=type=secret,id=sibling_fetch_token \
    set -eu; \
    if [ -s /run/secrets/sibling_fetch_token ]; then \
      git config --global credential.helper store; \
      printf 'https://x-access-token:%s@github.com\n' "$(cat /run/secrets/sibling_fetch_token)" > ~/.git-credentials; \
    fi; \
    cargo build --release --bin mcpg-inspector-tui; \
    rm -f ~/.git-credentials

# ----------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime
LABEL org.opencontainers.image.title="mcpg-inspector-tui" \
      org.opencontainers.image.description="MCP inspector for mcpg — interactive terminal client" \
      org.opencontainers.image.licenses="Apache-2.0"
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 mcpg
COPY --from=build /src/target/release/mcpg-inspector-tui /usr/local/bin/mcpg-inspector-tui
USER mcpg
WORKDIR /home/mcpg
ENTRYPOINT ["tini", "--", "mcpg-inspector-tui"]
