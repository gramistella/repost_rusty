# Use the lukemathwalker/cargo-chef image as the base
FROM docker.io/lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /repostrusty

FROM chef AS planner
COPY .env .env
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml
COPY ./instagram_scraper_rs ./instagram_scraper_rs
COPY ./src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /repostrusty/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
COPY ./instagram_scraper_rs ./instagram_scraper_rs
RUN cargo chef cook --release --recipe-path recipe.json

# Copy the source code and configuration
COPY .env .env
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml
COPY ./src ./src
COPY ./config/ui_definitions.yaml ./config/ui_definitions.yaml

# Build the application
RUN cargo build --release

# Final stage
FROM debian:bookworm-slim

# Install ffmpeg
RUN apt-get update && apt-get install -y ffmpeg libpq-dev

# Copy the built executable and configuration from the builder stage
# COPY --from=builder /repostrusty/config/ /repostrusty/config
COPY --from=builder /repostrusty/target/release/repost_rusty /repostrusty/target/release/repost_rusty

WORKDIR /repostrusty

# Set the startup command to run your binary
CMD ["/repostrusty/target/release/repost_rusty"]