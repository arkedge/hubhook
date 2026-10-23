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

FROM gcr.io/distroless/cc@sha256:d74d144baa124dd9defd01d71872bb56c49d4441dba6153ccf790364f97e64b1
WORKDIR app
COPY --from=builder /build/target/release/hubhook .
CMD ["/app/hubhook"]
