FROM rust:alpine AS builder

RUN apk add musl-dev

WORKDIR /build

COPY . .

RUN cargo build --release

FROM scratch

COPY --from=builder /build/target/release/aghast /usr/bin/aghast

EXPOSE 8080
ENTRYPOINT ["/usr/bin/aghast"]
