FROM rust:1.61.0 as chef
LABEL maintainer "sksat <sksat@arkedgespace.com>"

# depName=LukeMathWalker/cargo-chef datasource=github-releases
ARG CARGO_CHEF_VERSION="v0.1.36"
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

FROM gcr.io/distroless/cc@sha256:bac217056540e5330875164e5d6e29b2fcd2725ed0994332b6a8650d57ddd94d
WORKDIR app
COPY --from=builder /build/target/release/hubhook .
CMD ["/app/hubhook"]
