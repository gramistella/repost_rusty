# Use an official Rust runtime as a parent image
FROM docker.io/rust:latest

# Set the working directory in the container to /app
WORKDIR /repostrusty

# Copy the current directory contents into the container at /app
COPY . /repostrusty

# Build the application
RUN cargo build

# Set the startup command to run your binary
CMD ["./target/debug/repost_rusty"]