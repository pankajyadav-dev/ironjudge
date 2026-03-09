FROM rust:1.85-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p http_ironjudge

FROM debian:bookworm-slim AS runner
WORKDIR /app
ARG HTTPURL
ARG REDISURL
ARG STREAMNAME
ARG REDIS_POOL_SIZE
ENV HTTPURL=$HTTPURL
ENV REDISURL=$REDISURL
ENV STREAMNAME=$STREAMNAME
ENV REDIS_POOL_SIZE=$REDIS_POOL_SIZE

COPY --from=builder /app/target/release/http_ironjudge /app/http_ironjudge
CMD ["./http_ironjudge"]