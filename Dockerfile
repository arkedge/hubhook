FROM ghcr.io/sksat/cargo-chef-docker:1.78.0-bullseye as chef
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

FROM gcr.io/distroless/cc@sha256:3310655aac0d85eb9d579792387af1ff3eb7a1667823478be58020ab0e0d97a8
WORKDIR app
COPY --from=builder /build/target/release/hubhook .
CMD ["/app/hubhook"]
