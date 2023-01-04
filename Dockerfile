FROM rust:1.66.0 as chef
LABEL maintainer "sksat <sksat@arkedgespace.com>"

# depName=LukeMathWalker/cargo-chef datasource=github-releases
ARG CARGO_CHEF_VERSION="v0.1.50"
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

FROM gcr.io/distroless/cc@sha256:57acc9b6fe19e5a6b719068a9f5b8087266a1fdc49d5d79a5f0a11617ca1f4af
WORKDIR app
COPY --from=builder /build/target/release/hubhook .
CMD ["/app/hubhook"]
