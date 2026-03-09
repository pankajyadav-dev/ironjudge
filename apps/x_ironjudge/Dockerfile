FROM rust:1.85-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p x_ironjudge
FROM debian:bookworm-slim AS runner
WORKDIR /app

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates curl unzip default-jdk g++ rustc python3 && \
    rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://bun.sh/install | bash

RUN mkdir -p /app/sandbox_bin && \
    ln -s /usr/bin/java /app/sandbox_bin/java && \
    ln -s /usr/bin/javac /app/sandbox_bin/javac && \
    ln -s /usr/bin/rustc /app/sandbox_bin/rustc && \
    ln -s /usr/bin/g++ /app/sandbox_bin/g++ && \
    ln -s /root/.bun/bin/bun /app/sandbox_bin/bun

ENV PATH="/app/sandbox_bin:${PATH}"

ARG REDISURL
ARG STREAMNAME
ARG GROUPNAME
ARG CONSUMERNAME
ARG REDISPAYLOADLEN
ENV REDISURL=$REDISURL \
    STREAMNAME=$STREAMNAME \
    GROUPNAME=$GROUPNAME \
    CONSUMERNAME=$CONSUMERNAME \
    REDISPAYLOADLEN=$REDISPAYLOADLEN

COPY --from=builder /app/target/release/x_ironjudge /app/x_ironjudge
CMD ["./x_ironjudge"]