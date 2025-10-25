FROM rust:alpine AS builder

WORKDIR /app
RUN USER=root \
    && apk add --no-cache musl-dev pkgconfig openssl-dev openssl-libs-static

COPY . .

RUN cargo build --release

FROM alpine:latest

WORKDIR /app
RUN apk update && apk add openssl ca-certificates

COPY --from=builder /app/target/release/go-news-backend .

CMD ["./go-news-backend"]