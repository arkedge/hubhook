# Changelog

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.7.0] - 2026-09-09

More events are notified, more query fields can be matched, and the Slack
messages look different.

The version in `Cargo.toml` was left at 0.5.0 when 0.6.0 was released; it is
corrected to 0.7.0 here.

### Added

- Notify `pull_request_review` and `pull_request_review_comment` ([#339])
- `assignee`, `reviewer` and `review_state` queries, and review request
  notifications ([#340])
- Expand `@org/team` mentions to the member logins ([#341])
- Send bodies as Slack **markdown blocks**, so headings, tables and code blocks
  render as written ([#342])
- Link repository and account names, and put assignees on a single line ([#343])
- Show who did it (the sender) with their avatar in the attachment footer ([#349])

### Changed

- Update Rust to 1.90.0 and migrate to edition 2024 ([#321]), then to 1.97.1
  ([#344])
- Publish the image to GHCR instead of ECR: `ghcr.io/arkedge/hubhook` ([#348])

### Fixed

- Payload types were stricter than the real payloads, so deserialization failed
  and notifications were silently dropped ([#312], [#314], [#347])
- Dispatch payloads by `X-GitHub-Event` instead of guessing from the shape
  ([#314])
- Typo in the assignees heading ("Asiggnees") ([#293])

### Security

- Verify webhook signatures in constant time. The previous comparison returned
  on the first mismatch, so the signature could be guessed one byte at a time
  from the timing ([#346])

## [0.6.0] - 2024-10-30

### Added

- Report errors to Sentry ([#82])
- `exclude_query` rule to exclude matches ([#201])
- Healthcheck endpoint ([#266])
- Support issues with an empty body ([#67])
- Throw review requests to sksat ([#43])

### Changed

- Move the HTTP client from surf to reqwest ([#267])
- Update actix-web to 4.9.0 ([#265])
- Use sksat/cargo-chef-docker as the builder base image ([#133])

### Fixed

- The Pull Request payload struct did not match the real payload ([#68])
- Typo in milestone ([#78])

## [0.5.0] - 2022-02-16

First release.

[Unreleased]: https://github.com/arkedge/hubhook/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/arkedge/hubhook/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/arkedge/hubhook/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/arkedge/hubhook/releases/tag/v0.5.0

[#43]: https://github.com/arkedge/hubhook/pull/43
[#67]: https://github.com/arkedge/hubhook/pull/67
[#68]: https://github.com/arkedge/hubhook/pull/68
[#78]: https://github.com/arkedge/hubhook/pull/78
[#82]: https://github.com/arkedge/hubhook/pull/82
[#133]: https://github.com/arkedge/hubhook/pull/133
[#201]: https://github.com/arkedge/hubhook/pull/201
[#265]: https://github.com/arkedge/hubhook/pull/265
[#266]: https://github.com/arkedge/hubhook/pull/266
[#267]: https://github.com/arkedge/hubhook/pull/267
[#293]: https://github.com/arkedge/hubhook/pull/293
[#312]: https://github.com/arkedge/hubhook/pull/312
[#314]: https://github.com/arkedge/hubhook/pull/314
[#321]: https://github.com/arkedge/hubhook/pull/321
[#339]: https://github.com/arkedge/hubhook/pull/339
[#340]: https://github.com/arkedge/hubhook/pull/340
[#341]: https://github.com/arkedge/hubhook/pull/341
[#342]: https://github.com/arkedge/hubhook/pull/342
[#343]: https://github.com/arkedge/hubhook/pull/343
[#344]: https://github.com/arkedge/hubhook/pull/344
[#346]: https://github.com/arkedge/hubhook/pull/346
[#347]: https://github.com/arkedge/hubhook/pull/347
[#348]: https://github.com/arkedge/hubhook/pull/348
[#349]: https://github.com/arkedge/hubhook/pull/349
