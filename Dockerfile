ARG LLVMTARGETARCH
FROM --platform=${BUILDPLATFORM} ghcr.io/randomairborne/cross-cargo:${LLVMTARGETARCH} AS builder

ARG LLVMTARGETARCH

WORKDIR /build

COPY . .

RUN cargo build --release --target ${LLVMTARGETARCH}-unknown-linux-musl

FROM scratch
ARG LLVMTARGETARCH

COPY --from=builder /build/target/${LLVMTARGETARCH}-unknown-linux-musl/release/aghast /usr/bin/aghast

ENTRYPOINT ["/usr/bin/aghast"]
