# Kyte build and test image. Docker (not Podman-specific features).
# Targets: dev (default for `docker build --target dev`), build, runtime.
# On SELinux-enforcing hosts mount the source with the :Z suffix:
#   docker run --rm -v "$PWD":/src:Z -w /src kyte-dev cargo test
FROM rust:1 AS dev
RUN apt-get update && apt-get install -y --no-install-recommends \
      tmux ffmpeg fonts-dejavu-core ca-certificates curl git \
    && rm -rf /var/lib/apt/lists/*
RUN rustup component add rustfmt clippy
# asciinema agg renders .cast to .gif; if this step fails the demo still works without it
RUN cargo install --locked --git https://github.com/asciinema/agg || true
WORKDIR /src

FROM dev AS build
COPY . .
RUN cargo build --release

FROM debian:stable-slim AS runtime
COPY --from=build /src/target/release/kyte /usr/local/bin/kyte
ENTRYPOINT ["kyte"]
