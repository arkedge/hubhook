use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use tracing::{debug, error, info, warn};

/// Slack への POST 全体の予算。**再送する分も含める。**
///
/// GitHub の webhook 配信タイムアウト (10 秒) を超えると GitHub が再送し、
/// 通知が重複する。1 リクエストごとにタイムアウトを取り直すと、退避のための
/// 再送で予算が倍になってしまうので、締め切りを 1 つ決めて分け合う。
pub const POST_BUDGET: Duration = Duration::from_secs(5);

/// Slack API のベース URL。テストでモックに向けるために分けてある。
const API_BASE: &str = "https://slack.com";

/// 締め切りまでの残り時間。使い切っていれば `None`。
fn remaining(deadline: Instant, now: Instant) -> Option<Duration> {
    let left = deadline.saturating_duration_since(now);

    (!left.is_zero()).then_some(left)
}

#[derive(Debug)]
pub struct Message {
    pub text: String,
    pub attachments: Option<Vec<Attachment>>,
}

/// `chat.postMessage` の失敗。
#[derive(Debug)]
enum PostError {
    /// リクエスト自体が失敗した (タイムアウトなど)
    Request(String),
    /// Slack が API エラーを返した (HTTP 200 + `ok: false`)
    Api(String),
}

impl std::fmt::Display for PostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(e) => write!(f, "request failed: {e}"),
            Self::Api(e) => write!(f, "{e}"),
        }
    }
}

/// blocks が原因と考えられるエラーか。
///
/// コードは `chat.postMessage` の Errors に載っているものだけを書く
/// (<https://docs.slack.dev/reference/methods/chat.postMessage>)。
/// それらしい名前でも実在しないコードを書くと、その分岐は永久に通らない。
/// `invalid_attachments` は無く、長さの上限は `msg_too_long` ではなく
/// `msg_blocks_too_long`。
///
/// `invalid_auth` や `channel_not_found` は blocks を外しても直らないので、
/// 再送しても 2 回目が無駄に失敗し、レート制限を悪化させるだけ。
/// ここに無いエラーが blocks 由来だった場合はログに残るので、後から足せる。
///
/// `invalid_arguments` は blocks 以外が原因でも返る汎用のエラーだが、あえて
/// 含めている。attachment の中で markdown ブロックが使えるかはドキュメントに
/// 記載が無く、拒否されるとしてどのエラーで返るかも分からない。外して汎用の
/// エラーで返っていた場合、本文のある通知が全部無言で落ちる。含めた場合の
/// 損は API 1 回分で、しかも [`POST_BUDGET`] の中に収まる。
fn is_blocks_problem(error: &str) -> bool {
    matches!(
        error,
        "invalid_blocks" | "invalid_blocks_format" | "msg_blocks_too_long" | "invalid_arguments"
    )
}

/// 退避して再送すべきか。
///
/// blocks を外して直るのは blocks 由来のエラーだけで、しかも payload に
/// blocks が無ければ外しても何も変わらない。どちらも満たさない再送は
/// 2 回目も同じ結果になり、時間とレート制限を捨てるだけになる。
fn should_retry(payload: &MessagePayload, error: &str) -> bool {
    is_blocks_problem(error) && payload.has_blocks()
}

/// `chat.postMessage` の応答。
///
/// Slack は API エラーも HTTP 200 で返し、本文の `ok` で示す。
/// ステータスだけ見ていると `invalid_blocks` などに気付けない。
#[derive(Debug, Deserialize)]
struct PostResponse {
    ok: bool,
    error: Option<String>,
}

async fn post(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    payload: &MessagePayload,
    timeout: Duration,
) -> Result<(), PostError> {
    let res = client
        .post(format!("{base}/api/chat.postMessage"))
        .timeout(timeout)
        .bearer_auth(token)
        .json(payload)
        .send()
        .await
        .map_err(|e| PostError::Request(e.to_string()))?;

    let body: PostResponse = res
        .json()
        .await
        .map_err(|e| PostError::Request(format!("could not read response: {e}")))?;

    debug!("{body:?}");

    if body.ok {
        Ok(())
    } else {
        Err(PostError::Api(
            body.error.unwrap_or_else(|| "unknown error".to_string()),
        ))
    }
}

#[derive(Debug, Serialize)]
pub struct MessagePayload {
    pub channel: String,
    pub username: Option<String>,
    pub text: String,
    pub fallback: Option<String>,
    pub attachments: Option<Vec<Attachment>>,
    /// 本文中のリンクを unfurl させない。
    ///
    /// 既定では Slack が URL を展開してプレビューを付けるため、repo や
    /// アカウントをリンクにすると通知が縦に伸びて読みにくくなる。
    /// 既定値に依存せず明示的に切る。
    pub unfurl_links: bool,
    pub unfurl_media: bool,
}

#[derive(Debug, Serialize)]
pub struct Attachment {
    pub title: Option<String>,
    pub title_link: Option<url::Url>,
    pub fallback: String,
    pub color: Option<Color>,
    /// 誰の操作かを小さく出す
    #[serde(flatten)]
    pub footer: Option<Footer>,
    #[serde(flatten)]
    pub body: Body,
}

/// attachment の footer。
///
/// mrkdwn は効かないので素のテキスト (`mrkdwn_in` に footer は入れられない)。
/// 300 文字までで、狭い画面ではさらに切られる。
/// `footer_icon` は footer があるときだけ効き、16x16 で描画される。
///
/// <https://docs.slack.dev/legacy/legacy-messaging/legacy-secondary-message-attachments>
#[derive(Debug, Serialize)]
pub struct Footer {
    pub footer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer_icon: Option<url::Url>,
}

/// attachment の本文。
///
/// `blocks` と `text` の**どちらか一方**しか送らない。両方入れると Slack が
/// 両方を描画して本文が二重に出るので、型で片方に限っている
/// (`skip_serializing_if` は自分の値しか見られず、兄弟フィールドの有無では
/// 分岐できない)。
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Body {
    /// markdown ブロックとして送る。
    ///
    /// attachment の `text` は mrkdwn (Slack 独自記法) なので、GitHub の本文を
    /// そのまま貼ると崩れる (`##` がそのまま出る、`*x*` の強調が入れ替わる)。
    /// markdown ブロックは **本物の Markdown** を解釈するので、見出しや表、
    /// タスクリストまでそのまま渡せる。
    /// 色バーを残したいので、トップレベルではなく attachment の中に置く。
    Blocks {
        blocks: Vec<Block>,
        /// 拒否されたときの退避先 (mrkdwn)。リクエストには含めない。
        #[serde(skip)]
        mrkdwn: Option<String>,
    },
    /// 従来どおり attachment の `text` として送る (退避先)。
    Text { text: String },
    /// 本文が無い。
    ///
    /// 空の `text` を持つブロックは `invalid_blocks` で拒否され、通知そのものが
    /// 飛ばなくなるので、空なら何も入れない。
    Empty {},
}

impl Body {
    /// markdown ブロックと、拒否されたとき用の mrkdwn から作る。
    pub fn new(blocks: Vec<Block>, mrkdwn: Option<String>) -> Self {
        if !blocks.is_empty() {
            return Self::Blocks { blocks, mrkdwn };
        }

        match mrkdwn {
            Some(text) => Self::Text { text },
            None => Self::Empty {},
        }
    }

    /// blocks をやめて退避先に変える。
    fn fall_back(&mut self) {
        let Self::Blocks { mrkdwn, .. } = self else {
            return;
        };

        let mrkdwn = mrkdwn.take();
        *self = match mrkdwn {
            Some(text) => Self::Text { text },
            None => Self::Empty {},
        };
    }

    /// 中の blocks。テストで中身を確認するために使う。
    #[cfg(test)]
    pub fn blocks(&self) -> &[Block] {
        match self {
            Self::Blocks { blocks, .. } => blocks,
            _ => &[],
        }
    }

    /// 送る `text`。テストで中身を確認するために使う。
    #[cfg(test)]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }

    /// 退避用に用意しておいた mrkdwn。テストで中身を確認するために使う。
    #[cfg(test)]
    pub fn mrkdwn(&self) -> Option<&str> {
        match self {
            Self::Blocks { mrkdwn, .. } => mrkdwn.as_deref(),
            _ => None,
        }
    }
}

/// Block Kit のブロック。今は markdown だけ使う。
///
/// <https://docs.slack.dev/reference/block-kit/blocks/markdown-block>
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Markdown { text: String },
}

impl MessagePayload {
    /// blocks をやめて、`text` だけにした payload。
    ///
    /// markdown ブロックが受け付けられない場合の退避先。用意しておいた
    /// mrkdwn の本文を `text` に移す。ブロックの Markdown を流用すると
    /// `**太字**` や `[name](url)` が解釈されず、従来より悪い表示になる
    /// (方言が違う)。
    ///
    /// 従来の表現なので、長い本文は Slack 側で畳まれる。
    fn into_text_fallback(mut self) -> Self {
        for a in self.attachments.iter_mut().flatten() {
            a.body.fall_back();
        }

        self
    }

    /// 本文をどの表現で送ったか。ログに出して答え合わせに使う。
    ///
    /// attachment の中で markdown ブロックが使えるかはドキュメントに記載が無く、
    /// こちらでは確かめられない。実際に通ったかはログでしか分からない。
    fn body_kind(&self) -> &'static str {
        let bodies = || self.attachments.iter().flatten().map(|a| &a.body);

        if bodies().any(|b| matches!(b, Body::Blocks { .. })) {
            "markdown blocks"
        } else if bodies().any(|b| matches!(b, Body::Text { .. })) {
            "attachment text"
        } else {
            "no body"
        }
    }

    /// markdown ブロックを含むか。
    ///
    /// 含まないなら退避しても payload は変わらない。再送しても同じエラーで
    /// 確実に失敗するので、時間とレート制限を捨てるだけになる。
    fn has_blocks(&self) -> bool {
        self.attachments
            .iter()
            .flatten()
            .any(|a| matches!(a.body, Body::Blocks { .. }))
    }
}

impl Block {
    /// ブロックの本文。テストで中身を確認するために使う。
    #[cfg(test)]
    pub fn text(&self) -> &str {
        match self {
            Self::Markdown { text } => text,
        }
    }

    /// 本文からブロックを作る。
    ///
    /// 空なら `None`。空の `text` は `invalid_blocks` で拒否され、
    /// 通知そのものが飛ばなくなる。
    ///
    /// 長さは切らない。markdown ブロックには payload 全体で 12,000 文字の
    /// 上限があるが、超えた場合は Slack に拒否させて `text` へ退避する
    /// ([`MessagePayload::into_text_fallback`])。退避先では従来どおり
    /// Slack が長い本文を「Show more」で畳む。
    pub fn markdown(text: &str) -> Option<Self> {
        if text.trim().is_empty() {
            return None;
        }

        Some(Self::Markdown {
            text: text.to_string(),
        })
    }
}

// Slack attachment の色パレット。
// Closed は今のところ使っていないが、定義として残す
#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    Good,
    Warning,
    Danger,

    // GitHub
    #[serde(rename = "#24292F")]
    Comment,
    #[serde(rename = "#6F42C1")]
    Merged,
    #[serde(rename = "#CB2431")]
    Closed,
}

impl Message {
    #[allow(dead_code)]
    pub fn from_string(text: String) -> Self {
        Self {
            text,
            attachments: None,
        }
    }

    pub async fn post_message(self, token: &str, channel: &str, username: Option<&str>) {
        self.post_message_to(API_BASE, token, channel, username)
            .await
    }

    async fn post_message_to(self, base: &str, token: &str, channel: &str, username: Option<&str>) {
        // reqwest にはデフォルトのタイムアウトが無い。Slack が応答しないと
        // webhook のレスポンスを返せず、GitHub 側が再送して通知が重複する。
        // リクエストごとに残り時間を渡すが、渡し忘れの上限としても入れておく。
        let client = reqwest::Client::builder()
            .timeout(POST_BUDGET)
            .build()
            .expect("could not build http client");

        let deadline = Instant::now() + POST_BUDGET;

        let payload = MessagePayload {
            channel: channel.to_string(),
            username: username.map(|u| u.to_string()),
            text: self.text,
            fallback: None,
            attachments: self.attachments,
            unfurl_links: false,
            unfurl_media: false,
        };

        match post(&client, base, token, &payload, POST_BUDGET).await {
            Ok(()) => {
                debug!("POST ok ({})", payload.body_kind());
                return;
            }
            // リクエスト自体の失敗は payload を変えても直らない。
            // 再送すると待ち時間も倍になるので諦める。
            Err(PostError::Request(e)) => {
                error!("POST: {e}");
                return;
            }
            Err(PostError::Api(e)) => {
                if !should_retry(&payload, &e) {
                    error!("POST: {e}");
                    return;
                }

                // markdown ブロックが attachment 内で使えるか、本文が上限を
                // 超えたかはこちらで判定できない。blocks 由来と思われる
                // エラーなら、従来の表現 (attachment の text) で再送する。
                warn!("POST rejected ({e}); retrying without markdown blocks");
            }
        }

        // 再送も予算の中で行う。取り直すと webhook の締め切りを超えて
        // GitHub が再送し、通知が重複する
        let Some(left) = remaining(deadline, Instant::now()) else {
            error!("POST (fallback): out of budget");
            return;
        };

        let fallback = payload.into_text_fallback();
        match post(&client, base, token, &fallback, left).await {
            // 退避が起きたこと自体が知りたい情報なので debug では埋もれる
            Ok(()) => info!("POST ok (fallback: {})", fallback.body_kind()),
            Err(e) => error!("POST (fallback): {e}"),
        }
    }
}

//#[cfg(test)]
//#[actix_web::test]
//async fn test_post() {
//    post_message("xoxb-***", "tmp_hubhook", "test").await;
//}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(body: Body) -> Attachment {
        Attachment {
            title: None,
            title_link: None,
            fallback: "fallback".to_string(),
            color: None,
            footer: None,
            body,
        }
    }

    fn payload_with_footer(body: Body, footer: Option<Footer>) -> MessagePayload {
        let mut p = payload(body);
        p.attachments.as_mut().unwrap()[0].footer = footer;

        p
    }

    fn payload(body: Body) -> MessagePayload {
        MessagePayload {
            channel: "c".to_string(),
            username: None,
            text: "summary".to_string(),
            fallback: None,
            unfurl_links: false,
            unfurl_media: false,
            attachments: Some(vec![attachment(body)]),
        }
    }

    fn message(body: Body) -> Message {
        Message {
            text: "summary".to_string(),
            attachments: Some(vec![attachment(body)]),
        }
    }

    /// `chat.postMessage` を受けるテスト用サーバを立て、base URL と受け取った
    /// payload を返す。`replies` を順に返し、尽きたら成功を返す。
    ///
    /// 再送は「1 回目の応答を読んで 2 回目を投げる」という手順そのものが
    /// 本体なので、HTTP を実際に通さないと壊れても気付けない。
    fn spawn_slack(
        replies: Vec<serde_json::Value>,
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    ) {
        use actix_web::{App, HttpResponse, HttpServer, web};
        use std::sync::{Arc, Mutex};

        let got: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let replies = Arc::new(Mutex::new(replies));
        let got_srv = got.clone();

        let srv = HttpServer::new(move || {
            let got = got_srv.clone();
            let replies = replies.clone();

            App::new().route(
                "/api/chat.postMessage",
                web::post().to(move |body: web::Json<serde_json::Value>| {
                    let got = got.clone();
                    let replies = replies.clone();

                    async move {
                        got.lock().unwrap().push(body.into_inner());

                        let mut replies = replies.lock().unwrap();
                        let reply = if replies.is_empty() {
                            serde_json::json!({ "ok": true })
                        } else {
                            replies.remove(0)
                        };

                        HttpResponse::Ok().json(reply)
                    }
                }),
            )
        })
        .bind("127.0.0.1:0")
        .expect("could not bind test server");

        let addr = srv.addrs()[0];
        actix_web::rt::spawn(srv.run());

        (format!("http://{addr}"), got)
    }

    fn rejected(error: &str) -> Vec<serde_json::Value> {
        vec![serde_json::json!({ "ok": false, "error": error })]
    }

    fn blocks(md: &str, mrkdwn: Option<&str>) -> Body {
        Body::new(
            vec![Block::markdown(md).expect("ブロックが作られない")],
            mrkdwn.map(str::to_string),
        )
    }

    #[test]
    fn short_markdown_is_passed_through() {
        let md = "## 概要\n\n**重要** な `code` と [link](https://example.com)";
        assert_eq!(
            Block::markdown(md).unwrap().text(),
            md,
            "変換せずそのまま渡す"
        );
    }

    /// 長さは切らないこと。
    ///
    /// 上限を超えた場合は Slack に拒否させて text へ退避する。
    /// こちらで切ると、切り方を誤って Markdown を壊す危険がある。
    #[test]
    fn long_markdown_is_not_truncated() {
        let md = "a".repeat(20_000);
        assert_eq!(Block::markdown(&md).unwrap().text(), md);
    }

    /// 空の本文ではブロックを作らないこと。
    ///
    /// 空の `text` を持つブロックを送ると `invalid_blocks` で拒否され、
    /// 通知そのものが飛ばなくなる。
    #[test]
    fn empty_body_makes_no_block() {
        assert!(Block::markdown("").is_none());
        assert!(Block::markdown("   \n  ").is_none());
    }

    /// blocks 由来のエラーだけ再送すること。
    ///
    /// 認証やチャンネルの問題は blocks を外しても直らないので、
    /// 再送しても無駄打ちになりレート制限を悪化させる。
    #[test]
    fn only_block_errors_are_retried() {
        for e in [
            "invalid_blocks",
            "invalid_blocks_format",
            "msg_blocks_too_long",
            "invalid_arguments",
        ] {
            assert!(is_blocks_problem(e), "{e} は再送すべき");
        }

        for e in [
            "invalid_auth",
            "channel_not_found",
            "not_in_channel",
            "rate_limited",
        ] {
            assert!(!is_blocks_problem(e), "{e} は再送すべきでない");
        }
    }

    /// **初回のリクエストに `text` を入れないこと。**
    ///
    /// Slack は attachment の `text` と blocks を両方描画するので、一緒に送ると
    /// 成功時に本文が二重に出る。退避用の mrkdwn は持っていても送らない。
    #[test]
    fn first_request_sends_blocks_without_text() {
        let payload = payload(blocks("## body", Some("## body\n*Assignees*")));
        let json = serde_json::to_value(&payload).expect("直列化に失敗");
        let a = &json["attachments"][0];

        assert!(a["blocks"].is_array(), "blocks が無い: {a}");
        assert!(a.get("text").is_none(), "text が同送されている: {a}");
    }

    /// 退避後は `text` だけになること。
    ///
    /// 用意しておいた mrkdwn が入る。ブロックの Markdown を流用すると
    /// `**太字**` や `[name](url)` が解釈されず、従来より悪い表示になる。
    #[test]
    fn fallback_request_sends_text_without_blocks() {
        let mrkdwn = "## body\n*Assignees*\n<https://github.com/sksat|sksat>";
        let payload = payload(blocks("## body", Some(mrkdwn))).into_text_fallback();
        let json = serde_json::to_value(&payload).expect("直列化に失敗");
        let a = &json["attachments"][0];

        assert_eq!(a["text"], mrkdwn, "mrkdwn の text になっていない: {a}");
        assert!(a.get("blocks").is_none(), "blocks が残っている: {a}");
    }

    /// 本文が無いときは `text` も `blocks` も送らないこと。
    ///
    /// 空の `text` を持つブロックは `invalid_blocks` で拒否される。
    #[test]
    fn bodyless_attachment_sends_neither() {
        let json = serde_json::to_value(payload(Body::new(vec![], None))).expect("直列化に失敗");
        let a = &json["attachments"][0];

        assert!(a.get("text").is_none(), "text が入っている: {a}");
        assert!(a.get("blocks").is_none(), "blocks が入っている: {a}");
    }

    /// どの表現で送ったかを言えること。
    ///
    /// attachment 内で markdown ブロックが使えるかは検証できないので、
    /// 実際に通ったかを知る手段はこのログだけになる。
    #[test]
    fn body_kind_names_the_representation() {
        assert_eq!(
            payload(blocks("## body", Some("body"))).body_kind(),
            "markdown blocks"
        );
        assert_eq!(
            payload(Body::new(vec![], Some("body".to_string()))).body_kind(),
            "attachment text"
        );
        assert_eq!(payload(Body::new(vec![], None)).body_kind(), "no body");

        // 退避すると表現が変わることも言えていること
        let fallback = payload(blocks("## body", Some("body"))).into_text_fallback();
        assert_eq!(fallback.body_kind(), "attachment text");
    }

    /// footer が `footer` / `footer_icon` として出ること。
    ///
    /// `Option` を flatten しているので、直列化の形を確かめておく。
    #[test]
    fn footer_is_flattened() {
        let footer = Footer {
            footer: "sksat".to_string(),
            footer_icon: Some("https://example.com/avatar.png".parse().unwrap()),
        };

        let json = serde_json::to_value(payload_with_footer(blocks("## body", None), Some(footer)))
            .expect("直列化に失敗");
        let a = &json["attachments"][0];

        assert_eq!(a["footer"], "sksat", "{a}");
        assert_eq!(a["footer_icon"], "https://example.com/avatar.png", "{a}");
    }

    /// footer が無いときは何も出ないこと。
    #[test]
    fn no_footer_sends_nothing() {
        let json = serde_json::to_value(payload_with_footer(blocks("## body", None), None))
            .expect("直列化に失敗");
        let a = &json["attachments"][0];

        assert!(a.get("footer").is_none(), "footer が入っている: {a}");
        assert!(
            a.get("footer_icon").is_none(),
            "footer_icon が入っている: {a}"
        );
    }

    /// blocks 由来のエラーで、かつ blocks を持つときだけ再送すること。
    ///
    /// blocks が無い payload は退避しても変わらないので、2 回目も同じエラーで
    /// 確実に失敗する。
    #[test]
    fn only_block_errors_with_blocks_are_retried() {
        let with_blocks = payload(blocks("## body", None));

        assert!(should_retry(&with_blocks, "invalid_blocks"));
        assert!(
            !should_retry(&with_blocks, "invalid_auth"),
            "blocks 由来でないエラーで再送している"
        );

        for body in [
            Body::new(vec![], Some("body".to_string())),
            Body::new(vec![], None),
        ] {
            assert!(
                !should_retry(&payload(body), "invalid_blocks"),
                "blocks が無いのに再送している"
            );
        }
    }

    /// 予算を使い切っていたら再送しないこと。
    ///
    /// 取り直すと GitHub の webhook 配信タイムアウトを超え、GitHub が再送して
    /// 通知が重複する。
    #[test]
    fn exhausted_budget_leaves_no_time_for_the_fallback() {
        let now = Instant::now();

        assert_eq!(
            remaining(now + Duration::from_secs(2), now),
            Some(Duration::from_secs(2)),
            "残っているのに再送されない"
        );
        assert_eq!(remaining(now, now), None, "使い切ったのに再送される");
        assert_eq!(
            remaining(now, now + Duration::from_secs(1)),
            None,
            "超過したのに再送される"
        );
    }

    /// 通ったら 1 回で終わること。
    #[actix_web::test]
    async fn success_posts_once() {
        let (base, got) = spawn_slack(vec![]);

        message(blocks("## body", Some("body")))
            .post_message_to(&base, "token", "channel", None)
            .await;

        let got = got.lock().unwrap();
        assert_eq!(got.len(), 1, "余計に送っている");
        assert!(
            got[0]["attachments"][0]["blocks"].is_array(),
            "blocks で送っていない: {}",
            got[0]
        );
    }

    /// blocks 由来のエラーなら、従来の表現で再送すること。
    #[actix_web::test]
    async fn block_error_is_retried_as_text() {
        let (base, got) = spawn_slack(rejected("invalid_blocks"));

        message(blocks("## body", Some("*Assignees*: sksat")))
            .post_message_to(&base, "token", "channel", None)
            .await;

        let got = got.lock().unwrap();
        assert_eq!(got.len(), 2, "再送していない");

        let a = &got[1]["attachments"][0];
        assert_eq!(
            a["text"], "*Assignees*: sksat",
            "mrkdwn になっていない: {a}"
        );
        assert!(a.get("blocks").is_none(), "blocks が残っている: {a}");
    }

    /// blocks 由来でないエラーでは再送しないこと。
    #[actix_web::test]
    async fn other_errors_are_not_retried() {
        let (base, got) = spawn_slack(rejected("invalid_auth"));

        message(blocks("## body", Some("body")))
            .post_message_to(&base, "token", "channel", None)
            .await;

        assert_eq!(got.lock().unwrap().len(), 1, "無駄に再送している");
    }

    /// blocks を持たない payload では再送しないこと。
    #[actix_web::test]
    async fn blockless_payloads_are_not_retried() {
        let (base, got) = spawn_slack(rejected("invalid_blocks"));

        message(Body::new(vec![], None))
            .post_message_to(&base, "token", "channel", None)
            .await;

        assert_eq!(got.lock().unwrap().len(), 1, "無駄に再送している");
    }

    /// ブロックの無い本文は退避しても変わらないこと。
    #[test]
    fn fallback_leaves_blockless_bodies_alone() {
        let mut empty = Body::new(vec![], None);
        empty.fall_back();
        assert!(matches!(empty, Body::Empty {}), "{empty:?}");

        let mut text = Body::new(vec![], Some("body".to_string()));
        text.fall_back();
        assert_eq!(text.text(), Some("body"));
    }
}
