FROM rust:1.89 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bullseye-slim
RUN apt-get update && apt-get install -y build-essential libssl-dev && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/go-news-backend .
CMD ["./go-news-backend"]