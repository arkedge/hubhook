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

FROM gcr.io/distroless/cc@sha256:c1cbcec08d39c81adbefb80cabc51cba285465866f7b5ab15ddb2fcae51a1aed
WORKDIR app
COPY --from=builder /build/target/release/hubhook .
CMD ["/app/hubhook"]
