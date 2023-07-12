FROM ghcr.io/sksat/cargo-chef-docker:1.70.0-bullseye as chef
LABEL maintainer "sksat <sksat@arkedgespace.com>"

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

FROM gcr.io/distroless/cc@sha256:b53fbf5f81f4a120a489fedff2092e6fcbeacf7863fce3e45d99cc58dc230ccc
WORKDIR app
COPY --from=builder /build/target/release/hubhook .
CMD ["/app/hubhook"]
