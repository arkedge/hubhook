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

### Message appearance

本文は Slack の **markdown ブロック**として送る。attachment の `text` は
mrkdwn (Slack 独自記法) なので、GitHub の本文をそのまま貼ると `##` が
そのまま表示され、`*x*` の強調も入れ替わる (GitHub は斜体、Slack は太字)。
markdown ブロックは本物の Markdown を解釈するので、見出し・表・タスクリスト・
コードブロックまでそのまま渡せる。色バーを残すため attachment の中に置いている。

GFM 固有の参照記法 (`#123` の issue リンク、`@user`、コミット SHA) は
Markdown の仕様外なのでリンクにはならない。

本文は payload 全体で 12,000 **文字**までなので、超える分は切って
`_(truncated)_` を付ける (バイト数で測ると日本語が上限の 1/3 の文字数で
切られてしまう)。Assignees は切り詰めで消えないよう、先に場所を確保する。
コードフェンスの途中で切った場合は閉じ記号を補う (閉じないと、印と
Assignees が未終了のコードブロックに飲まれる)。

本文が無い場合はブロックを作らない。空の `text` を持つブロックは
`invalid_blocks` で拒否され、通知そのものが飛ばなくなる。

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
`@Octocoders/octo-team` へのメンションで、`body` に `@sksat` を指定している
個人のルールにもマッチするようになる。

`body` クエリは次の 3 つに当てて、いずれかが一致すればマッチとする。

1. **元の本文** — `@org/team$` のようなアンカー付きルールの意味を保つ
   (連結したものだけに当てると末尾一致が効かなくなり、exclude_query では
   除外されるべきものが除外されなくなる)
2. **元の本文 + 展開結果** — 本文の文脈と組み合わせたパターン
   (`レビュー.*@sksat` など) を拾うため、本文を含めて連結する
3. **展開された `@login` を 1 つずつ** — まとめて 1 つの文字列に当てると、
   `@sksat$` のようなアンカー付きルールが並び順に依存してしまうため

既知の制限として、「本文の文脈 + member への末尾アンカー」
(`レビュー.*@sksat$` など) は展開結果の並び順に依存する。
本文を member ごとに連結して照合すれば解消するが、rule ごと × member ごとに
本文長を走査することになり、rule が増えるほど webhook 1 通の処理が重くなる。

`body` クエリを使う rule が 1 つも無い場合は、展開結果が使われないので
GitHub API を叩かない。

展開には `GITHUB_TOKEN` が必要 (org の team を読める権限)。
未設定の場合は展開されず、team メンションは team メンションのままとして扱う。

展開は **`GITHUB_TOKEN` の可視性**で行う。body を書いた人の権限は見ないので、
GitHub 上ではメンションできない secret team でも、team 名を書けば展開される。
通知先は Slack で、隠したい team もない前提なので、
「通知が飛ばない」より「飛ぶ」side に倒している (意図した挙動)。

同じ方針で、API の取得に失敗した場合も展開せずに処理を続ける
(fail-open)。team メンション以外のルールの通知を止めないため。

メンバーは 10 分キャッシュする。取得に失敗した場合は展開せずに処理を続け、
log と Sentry に記録する (他のルールの通知は止めない)。

webhook のレスポンスを遅らせないため、API 呼び出しには次の制限をかけている。

- 1 リクエスト 3 秒でタイムアウト
- 展開全体で 3 秒を超えたら打ち切る (team もページも直列に引くため、
  ページを進める前にも残り時間を見る)
- 1 つの body で展開する team は 8 件まで
- 失敗した team は 60 秒間再取得しない
- キャッシュは insert 時に期限切れを掃除し、最大 1024 team まで
- 同じ team に同時にリクエストが来ても API 呼び出しは 1 本にまとめる
  (キャッシュが空の瞬間は全員がミスするため、キャッシュだけでは防げない)

メンバーが 2000 人を **超える** team はエラーとして扱い、展開しない。
一部だけ返すと、載らなかった人に通知が飛ばないまま気付けないため。

なお、この 3 秒は **展開フェーズだけの上限**で、webhook 全体の締め切りでは
ない。展開のあとに channel ごとの Slack POST が直列で走る (それぞれ 5 秒で
タイムアウト) ため、channel が複数あると合計は GitHub の配信タイムアウト
(10 秒) を超えうる。超えると再送され、通知が重複する。
端から端まで縛るには、1 つの締め切りを配信まで通すか、配信を webhook の
応答から外す必要がある。

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
