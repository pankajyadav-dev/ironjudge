FROM rust:1.85-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p x_ironjudge

FROM debian:bookworm-slim AS sandbox-rootfs
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 default-jre-headless libstdc++6 libc6 && \
    rm -rf /var/lib/apt/lists/*
RUN apt-get update && apt-get install -y curl unzip && \
    curl -fsSL https://bun.sh/install | bash && \
    mv ~/.bun/bin/bun /usr/local/bin/bun && \
    apt-get purge -y curl unzip && apt-get autoremove -y && \
    rm -rf /var/lib/apt/lists/*
FROM debian:bookworm-slim AS runner
WORKDIR /app

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates curl unzip default-jdk-headless g++ rustc python3 tini&& \
    rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://bun.sh/install | bash && \
    apt-get purge -y curl unzip && apt-get autoremove -y && \
    rm -rf /root/.bun/install

RUN mkdir -p /app/sandbox_bin && \
    ln -s /usr/bin/java /app/sandbox_bin/java && \
    ln -s /usr/bin/javac /app/sandbox_bin/javac && \
    ln -s /usr/bin/rustc /app/sandbox_bin/rustc && \
    ln -s /usr/bin/g++ /app/sandbox_bin/g++ && \
    ln -s /root/.bun/bin/bun /app/sandbox_bin/bun

RUN echo '#include <bits/stdc++.h>\n#include <unistd.h>\n#include <fcntl.h>' > /usr/include/competition.h
RUN g++ -O2 -x c++-header /usr/include/competition.h -o /usr/include/competition.h.gch

RUN mkdir -p /opt/sandbox_rootfs
COPY --from=sandbox-rootfs /usr /opt/sandbox_rootfs/usr
COPY --from=sandbox-rootfs /etc /opt/sandbox_rootfs/etc

RUN cd /opt/sandbox_rootfs && \
    ln -s usr/bin bin && \
    ln -s usr/sbin sbin && \
    ln -s usr/lib lib && \
    ln -s usr/lib64 lib64

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
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["./x_ironjudge"]
