# hubhook
[![Rust](https://github.com/arkedge/hubhook/actions/workflows/rust.yml/badge.svg)](https://github.com/arkedge/hubhook/actions/workflows/rust.yml)
[![build / container image](https://github.com/arkedge/hubhook/actions/workflows/build-image.yml/badge.svg)](https://github.com/arkedge/hubhook/actions/workflows/build-image.yml)
[![license](https://img.shields.io/github/license/arkedge/hubhook)](https://github.com/arkedge/hubhook/blob/main/LICENSE)

GitHub notification manager

This project is inspired by [tokite](https://github.com/cookpad/tokite).

## Deploy

The image is `ghcr.io/arkedge/hubhook`, tagged with `main` for the tip of the
default branch, `sha-<short>` for every build, and the version plus `latest`
for a release.

|Environment variable|Description|
|-|-|
|`HUBHOOK_PORT`|port to listen on|
|`SLACK_TOKEN`|Slack bot token|
|`WEBHOOK_SECRET`|secret used to verify the webhook signature|
|`SENTRY_DSN`|Sentry DSN|
|`GITHUB_TOKEN`|used to expand team mentions (optional)|
|`CONFIG_PATH`|where the config lives (optional, defaults to `/config/config.json`)|
|`RUST_LOG`|log level and filter (optional, defaults to `info`)|

Only `HUBHOOK_PORT`, `SLACK_TOKEN`, `WEBHOOK_SECRET` and `SENTRY_DSN` are
required. `docker-compose.yml` and `.env.example` are set up for running it
locally.

## Configuration

Write rules in the config file (`CONFIG_PATH`). An event is posted to a rule's
`channel` when **every** field of its `query` matches. If the rule has an
`exclude_query`, the event is dropped when **any** of its fields matches.

```json
{
  "rule": [
    {
      "channel": "memo_sksat-hubhook",
      "query": {
        "topic": "arkedge|hoge-sat"
      }
    }
  ]
}
```

Values are case-insensitive regular expressions.

|Name|Description|
|-|-|
|repo|repository name|
|topic|repository topic|
|user|event sender|
|title|Issue title|
|body|body of an Issue, Issue Comment, review or review comment|
|label|Issue label|
|assignee|login of an Issue / PR assignee|
|reviewer|login of a requested reviewer, or the slug of a requested team|
|review_state|`pull_request_review` state (`approved` / `changes_requested` / `commented`)|

## Notified events

These `X-GitHub-Event` values are handled. Nothing is notified unless the
GitHub App / Webhook is subscribed to them.

- `issues`
- `issue_comment`
- `pull_request`
- `pull_request_review` (approve / changes requested / review with a comment)
- `pull_request_review_comment` (comments on a diff and their replies)

## Message appearance

Bodies are converted from GitHub Flavored Markdown to Slack mrkdwn, so bold,
italics, strikethrough, links, lists, task lists, quotes and code blocks
render. Slack has neither headings nor tables, so a heading becomes a bold line
and each table row becomes one line. GFM-specific references (`#123`, `@user`,
commit SHAs) are outside the Markdown spec and are not linked.

Repository and account names are links. Assignees are put on a single line as
`Assignees: a, b, c`. The attachment footer shows the login and avatar of
whoever did it (the sender).

## Team mention

When a body mentions `@org/team`, the team members are fetched from the GitHub
API and expanded to `@login` before the `body` query is matched. A mention of
`@Octocoders/octo-team` therefore also matches a personal rule whose `body` is
`@sksat`.

- Expansion needs `GITHUB_TOKEN`. Without it, nothing is expanded
- Expansion uses **the visibility of `GITHUB_TOKEN`**, not the permissions of
  whoever wrote the body. A secret team that cannot be mentioned on GitHub is
  still expanded if its name is written
- A failed API call does not stop the expansion phase, so rules other than team
  mentions still notify
- Members are cached for 10 minutes
- Teams with more than 2000 members are not expanded, to avoid notifying only
  some of them

A rule that combines body context with an anchor on a member, such as
`review.*@sksat$`, depends on the order of the expanded logins and may not
match.
