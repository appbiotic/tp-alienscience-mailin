FROM rust:latest as build-env
WORKDIR /app
COPY . /app
RUN mkdir -p /mailin/logs
RUN cargo build --release

FROM gcr.io/distroless/cc-debian12
EXPOSE 8025
COPY --from=build-env /app/target/release/mailin-server /
COPY --from=build-env /mailin /

CMD ["./mailin-server", \
      "--address","0.0.0.0:8025", \
      "--log","/mailin/logs", \
      "--maildir","/mailin/maildir", \
      "--ssl-cert","/mailin/certs/tls.crt", \
      "--ssl-key","/mailin/certs/tls.key", \
      "--blocklist","zen.spamhaus.org" \
    ]
