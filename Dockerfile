# Use an official Rust runtime as a parent image
FROM docker.io/rust:latest

# Set the working directory in the container to /app
WORKDIR /repostrusty

# Copy over your Manifest files
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml

# Copy your local crate
COPY ./instagram_scraper_rs ./instagram_scraper_rs

# This dummy build is to get the dependencies cached
RUN mkdir src && \
    echo "fn main() {println!(\"if you see this, the build broke\")}" > src/main.rs && \
    cargo build --release && \
    rm -rf src/

# Now that the dependencies are cached, copy your source code
COPY . .

# Build the application
RUN cargo build

# Start a new stage
# FROM debian:bookworm-slim

# Install necessary packages
RUN apt-get update && apt-get install -y ffmpeg

# Set the startup command to run your binary
CMD ["/repostrusty/target/debug/repost_rusty"]