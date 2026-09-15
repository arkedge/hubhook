FROM ghcr.io/sksat/cargo-chef-docker:1.97.1-bookworm as chef
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

FROM gcr.io/distroless/cc@sha256:4594d59540d1948417f6ca2829ddd9294493a7c68b7528f4dd459de7f203a750
WORKDIR app
COPY --from=builder /build/target/release/hubhook .
CMD ["/app/hubhook"]
