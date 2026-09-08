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
|assignee|Issue / PR の assignee の login|
|reviewer|review を依頼された user の login、または team の slug|
|review_state|`pull_request_review` の state (`approved` / `changes_requested` / `commented`)|

### Notified events

`X-GitHub-Event` のうち以下を扱う。
GitHub App / Webhook 側でこれらのイベントを購読していないと通知は飛ばない。

- `issues`
- `issue_comment`
- `pull_request`
- `pull_request_review` (approve / changes requested / コメント付き review)
- `pull_request_review_comment` (diff 上のコメントとその返信)

### Team mention

`body` に team メンション (`@org/team`) が書かれている場合、
GitHub API で team のメンバーを引いて `@login` に展開する (#286)。
`@arkedge/sat-sw` へのメンションで、`body` に `@sksat` を指定している
個人のルールにもマッチするようになる。

`body` クエリは「元の本文」と「展開結果」に別々に当てて、どちらかが
一致すればマッチとする。1 つの文字列に連結すると、`@org/team$` のような
アンカー付きの既存ルールの意味が変わってしまうため
(exclude_query 側では、除外されるべきものが除外されなくなる)。

展開には `GITHUB_TOKEN` が必要 (org の team を読める権限)。
未設定の場合は展開されず、team メンションは team メンションのままとして扱う。

メンバーは 10 分キャッシュする。取得に失敗した場合は展開せずに処理を続け、
log と Sentry に記録する (他のルールの通知は止めない)。

webhook のレスポンスを遅らせないため、API 呼び出しには次の制限をかけている。

- 1 リクエスト 3 秒でタイムアウト
- 展開全体で 5 秒を超えたら打ち切る (team もページも直列に引くため、
  ページを進める前にも残り時間を見る)
- 1 つの body で展開する team は 8 件まで
- 失敗した team は 60 秒間再取得しない
- キャッシュは insert 時に期限切れを掃除し、最大 1024 team まで
- 同じ team に同時にリクエストが来ても API 呼び出しは 1 本にまとめる
  (キャッシュが空の瞬間は全員がミスするため、キャッシュだけでは防げない)

メンバーが 2000 人を **超える** team はエラーとして扱い、展開しない。
一部だけ返すと、載らなかった人に通知が飛ばないまま気付けないため。

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
