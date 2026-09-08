# hubhook
[![Rust](https://github.com/arkedge/hubhook/actions/workflows/rust.yml/badge.svg)](https://github.com/arkedge/hubhook/actions/workflows/rust.yml)
[![build / container image](https://github.com/arkedge/hubhook/actions/workflows/build-image.yml/badge.svg)](https://github.com/arkedge/hubhook/actions/workflows/build-image.yml)
[![license](https://img.shields.io/github/license/arkedge/hubhook)](https://github.com/arkedge/hubhook/blob/main/LICENSE)

GitHub notification manager

This project is inspired by [tokite](https://github.com/cookpad/tokite).

## Deploy

## Configuration

Edit config.json.

### Supported query

|Name|Description|
|-|-|
|repo|repository name|
|topic|repository topic|
|user|event sender|
|title|Issue title|
|body|Issue / Issue Comment / review / review comment の本文|
|label|Issue label|
|review_state|`pull_request_review` の state (`approved` / `changes_requested` / `commented`)|

### Notified events

`X-GitHub-Event` のうち以下を扱う。
GitHub App / Webhook 側でこれらのイベントを購読していないと通知は飛ばない。

- `issues`
- `issue_comment`
- `pull_request`
- `pull_request_review` (approve / changes requested / コメント付き review)
- `pull_request_review_comment` (diff 上のコメントとその返信)

### Example
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
