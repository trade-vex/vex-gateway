# The read-model test stack's traffic driver, built from this repository (the build context is its
# root) so that it reaches vex-core through the gateway's own pinned vex-networking.
FROM rust:1.89.0-trixie@sha256:57407b378b2b6e07b48a6135a20c87cc22ea6e249c0acf6cb1833ead3cf116e9 AS build
# rusteron builds the Aeron C libraries with CMake 3.30+, libclang (bindgen) and a JDK.
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake clang libclang-dev libbsd-dev uuid-dev default-jdk-headless \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p readmodel-traffic \
    && mkdir /aeron \
    && find target/release/build -name 'libaeron*.so' -exec cp -t /aeron {} +

FROM debian:trixie-20261020-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
COPY --from=build /aeron/ /usr/local/lib/
RUN ldconfig
COPY --from=build /src/target/release/readmodel-traffic /usr/local/bin/readmodel-traffic
ENTRYPOINT ["/usr/local/bin/readmodel-traffic"]
