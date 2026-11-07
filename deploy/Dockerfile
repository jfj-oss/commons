# Build and run the Commons relay on a rented server.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY app/src-tauri/Cargo.toml app/src-tauri/Cargo.toml
# the workspace lists the desktop app; the relay build does not need it
RUN mkdir -p app/src-tauri/src && echo 'fn main(){}' > app/src-tauri/src/main.rs && echo 'fn main(){}' > app/src-tauri/build.rs
RUN cargo build --release -p commons-relay

FROM debian:bookworm-slim
RUN useradd --system --create-home --home-dir /data commons && mkdir -p /data && chown commons /data
COPY --from=build /src/target/release/commons-relay /usr/local/bin/commons-relay
USER commons
VOLUME /data
EXPOSE 8787
# 0.0.0.0 here is the container's private network; compose does NOT publish this port, only Caddy is public.
CMD ["commons-relay", "--bind", "0.0.0.0:8787", "--data", "/data"]
