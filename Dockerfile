FROM rust:1.68.0 as chef
LABEL maintainer "sksat <sksat@arkedgespace.com>"

# depName=LukeMathWalker/cargo-chef datasource=github-releases
ARG CARGO_CHEF_VERSION="v0.1.51"
RUN cargo install --version "${CARGO_CHEF_VERSION#v}" --locked cargo-chef
WORKDIR build

FROM chef as planner
COPY . .
RUN cargo chef prepare  --recipe-path recipe.json

# build
FROM chef as builder
COPY --from=planner /build/recipe.json recipe.json
# build deps(cached)
RUN cargo chef cook --release --recipe-path recipe.json
# build bin
COPY . .
RUN cargo build --release

FROM gcr.io/distroless/cc@sha256:8aad707f96620ee89e27febef51b01c6ff244277a3560fcfcfbe68633ef09193
WORKDIR app
COPY --from=builder /build/target/release/hubhook .
CMD ["/app/hubhook"]
