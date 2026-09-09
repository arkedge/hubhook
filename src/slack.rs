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

/// 退避しても直らないと分かっているエラー。
///
/// 認証・チャンネル・権限・レート制限は payload の形と無関係なので、中身を
/// 変えて送り直しても同じ結果になる。
///
/// エラー一覧は <https://docs.slack.dev/reference/methods/chat.postMessage>。
fn is_hopeless(error: &str) -> bool {
    matches!(
        error,
        // token
        "invalid_auth"
            | "not_authed"
            | "account_inactive"
            | "token_revoked"
            | "token_expired"
            | "not_allowed_token_type"
            | "two_factor_setup_required"
            // 権限・アクセス
            | "access_denied"
            | "no_permission"
            | "missing_scope"
            | "app_access_restricted"
            | "enterprise_is_restricted"
            | "ekm_access_denied"
            | "team_access_not_granted"
            | "org_login_required"
            | "send_on_behalf_not_allowed"
            | "messages_tab_disabled"
            // 宛先
            | "channel_not_found"
            | "not_in_channel"
            | "is_archived"
            | "team_not_found"
            | "team_added_to_org"
            | "restricted_action"
            | "restricted_action_read_only_channel"
            | "restricted_action_thread_only_channel"
            | "restricted_action_non_threadable_channel"
            | "restricted_action_thread_locked"
            // 流量
            | "ratelimited"
            | "rate_limited"
            | "accesslimited"
            | "message_limit_exceeded"
            // 呼び出し方
            | "deprecated_endpoint"
            | "method_deprecated"
            // attachment の数は退避しても変わらない
            | "too_many_attachments"
    )
}

/// 同じ payload を送り直してよいエラー。
///
/// `chat.postMessage` に冪等キーは無いので、既に投稿されている可能性がある
/// なら送り直せない。`internal_error` と `fatal_error` はドキュメントに
/// "It's possible some aspect of the operation succeeded before the error was
/// raised." と書かれているので除く。送り直すと通知が重複する。
///
/// `request_timeout` は名前に反して "the POST data was either missing or
/// truncated" で、送った内容の不備なので送り直しても直らない。
///
/// 残るのは `service_unavailable` ("The service is temporarily unavailable")
/// だけ。処理に入る前に断られているので、同じものを送ってよい。
///
/// エラー一覧は <https://docs.slack.dev/reference/methods/chat.postMessage>。
fn is_retriable(error: &str) -> bool {
    matches!(error, "service_unavailable")
}

/// 退避すべきか。ブロックを外した別の payload を送る。
///
/// 拒否されたときは投稿されていないので、別のものを送っても重複しない。
/// blocks を持たない payload は外しても変わらないので送らない。
///
/// どのエラーで拒否されるかはドキュメントに書かれていないので、[`is_hopeless`]
/// に無いものは退避してみる。漏れたときの損は API 1 回分で、[`POST_BUDGET`]
/// の中に収まる。
///
/// `internal_error` と `fatal_error` は "It's possible some aspect of the
/// operation succeeded before the error was raised." とされているので、
/// 厳密には投稿済みかどうか分からない。それでも退避する。
///
/// - attachment の中の markdown ブロックは、最小の payload でも
///   `internal_error` で拒否される (実測)
/// - その状態では本文のある通知が 1 通も届かなかった。部分成功していたなら
///   届いていたはずなので、この payload の形では拒否を意味する
/// - 外すと本文のある通知が全部落ちる。理論上の重複より、確実な取りこぼしの
///   方が損が大きい
///
/// ブロックを使わなくなればこの判断自体が要らなくなる。
fn should_fall_back(payload: &MessagePayload, error: &str) -> bool {
    payload.has_blocks() && !is_hopeless(error)
}

/// 1 通目が失敗したときに次に何を送るか。
///
/// 同じものを送る (リトライ) のと、表現を落として送る (退避) を混ぜると、
/// 処理前に断られただけの場合まで表現が落ちる。
enum Next {
    Retry,
    FallBack,
}

/// `chat.postMessage` の応答。
///
/// Slack は API エラーも HTTP 200 で返し、本文の `ok` で示す。
/// ステータスだけ見ていると `invalid_blocks` などに気付けない。
#[derive(Debug, Deserialize)]
struct PostResponse {
    ok: bool,
    error: Option<String>,
    /// Slack は `ok: true` でも警告を返す。`missing_charset` のように
    /// こちらの送り方の問題を指すものがあるので捨てない。
    warning: Option<String>,
    response_metadata: Option<ResponseMetadata>,
}

#[derive(Debug, Deserialize)]
struct ResponseMetadata {
    #[serde(default)]
    warnings: Vec<String>,
}

impl PostResponse {
    /// `warning` と `response_metadata.warnings` を混ぜて返す。
    ///
    /// 同じ内容が両方に入ることがあるので重複は落とす。
    fn warnings(&self) -> Vec<&str> {
        let flat = self
            .warning
            .as_deref()
            .into_iter()
            .flat_map(|w| w.split(','))
            .map(str::trim);
        let listed = self
            .response_metadata
            .as_ref()
            .into_iter()
            .flat_map(|m| m.warnings.iter().map(String::as_str));

        let mut out: Vec<&str> = Vec::new();
        for w in flat.chain(listed) {
            if !w.is_empty() && !out.contains(&w) {
                out.push(w);
            }
        }
        out
    }
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
        // reqwest の json() は charset を付けないため、Slack が
        // missing_charset を warning で返す。先に入れておくと json() は
        // Content-Type を上書きしない。
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )
        .json(payload)
        .send()
        .await
        .map_err(|e| PostError::Request(e.to_string()))?;

    let body: PostResponse = res
        .json()
        .await
        .map_err(|e| PostError::Request(format!("could not read response: {e}")))?;

    debug!("{body:?}");

    // ok: true でも返ってくる。こちらの送り方の問題を教えてくれるので出す。
    let warnings = body.warnings();
    if !warnings.is_empty() {
        // channel ごとに投稿するので、どの宛先の警告か分からないと追えない。
        // join せず配列のまま出す。確保が増えるし、構造も失われる
        warn!(
            channel = %payload.channel,
            ok = body.ok,
            warnings = ?warnings,
            "Slack returned warnings"
        );
    }

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

        let next = match post(&client, base, token, &payload, POST_BUDGET).await {
            Ok(()) => {
                // どの表現で通ったかは、表現を変えたときの答え合わせに要る。
                info!(channel, body = payload.body_kind(), "POST ok");
                return;
            }
            // リクエスト自体の失敗は payload を変えても直らない。
            // 再送すると待ち時間も倍になるので諦める。
            Err(PostError::Request(e)) => {
                error!(channel, error = %e, "POST failed");
                return;
            }
            Err(PostError::Api(e)) => {
                // 処理前に断られたなら、表現を落とす理由が無いので同じものを
                // 送る。先に退避を判定すると、この場合まで表現が落ちる。
                if is_retriable(&e) {
                    warn!(channel, error = %e, "POST failed; retrying");
                    Next::Retry
                } else if should_fall_back(&payload, &e) {
                    // markdown ブロックが attachment 内で使えるか、本文が上限を
                    // 超えたかはこちらで判定できない。拒否されたら、従来の
                    // 表現 (attachment の text) に落として送る。
                    warn!(channel, error = %e, "POST rejected; falling back");
                    Next::FallBack
                } else {
                    // 諦めるが無音にはしない。channel と link が残っていれば
                    // 落ちた通知を後から追える。
                    error!(channel, error = %e, "POST failed");
                    return;
                }
            }
        };

        // 2 通目も予算の中で送る。取り直すと webhook の締め切りを超えて
        // GitHub が再送し、通知が重複する
        let Some(left) = remaining(deadline, Instant::now()) else {
            error!(channel, "POST retry skipped: out of budget");
            return;
        };

        let fallback = match next {
            Next::Retry => payload,
            Next::FallBack => payload.into_text_fallback(),
        };
        match post(&client, base, token, &fallback, left).await {
            // 届いてはいるが本来の表現が拒否された、という degraded success。
            Ok(()) => warn!(channel, body = fallback.body_kind(), "POST ok (fallback)"),
            Err(e) => error!(channel, error = %e, "POST failed (fallback)"),
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
    type Captured<T> = std::sync::Arc<std::sync::Mutex<Vec<T>>>;

    fn spawn_slack(
        replies: Vec<serde_json::Value>,
    ) -> (String, Captured<serde_json::Value>, Captured<String>) {
        use actix_web::{App, HttpRequest, HttpResponse, HttpServer, web};
        use std::sync::{Arc, Mutex};

        let got: Captured<serde_json::Value> = Arc::new(Mutex::new(Vec::new()));
        let ctypes: Captured<String> = Arc::new(Mutex::new(Vec::new()));
        let replies = Arc::new(Mutex::new(replies));
        let got_srv = got.clone();
        let ctypes_srv = ctypes.clone();

        let srv = HttpServer::new(move || {
            let got = got_srv.clone();
            let ctypes = ctypes_srv.clone();
            let replies = replies.clone();

            App::new().route(
                "/api/chat.postMessage",
                web::post().to(
                    move |req: HttpRequest, body: web::Json<serde_json::Value>| {
                        let got = got.clone();
                        let ctypes = ctypes.clone();
                        let replies = replies.clone();

                        async move {
                            got.lock().unwrap().push(body.into_inner());
                            ctypes.lock().unwrap().push(
                                req.headers()
                                    .get("content-type")
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("")
                                    .to_string(),
                            );

                            let mut replies = replies.lock().unwrap();
                            let reply = if replies.is_empty() {
                                serde_json::json!({ "ok": true })
                            } else {
                                replies.remove(0)
                            };

                            HttpResponse::Ok().json(reply)
                        }
                    },
                ),
            )
        })
        .bind("127.0.0.1:0")
        .expect("could not bind test server");

        let addr = srv.addrs()[0];
        actix_web::rt::spawn(srv.run());

        (format!("http://{addr}"), got, ctypes)
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

    /// 直らないと分かっているエラーだけ再送しないこと。
    ///
    /// 認証やチャンネルの問題は blocks を外しても直らないので、再送しても
    /// 無駄打ちになりレート制限を悪化させる。
    #[test]
    fn hopeless_errors_are_not_retried() {
        for e in [
            "invalid_auth",
            "token_expired",
            "channel_not_found",
            "not_in_channel",
            "is_archived",
            "missing_scope",
            "restricted_action",
            "restricted_action_read_only_channel",
            "team_access_not_granted",
            "ekm_access_denied",
            "ratelimited",
            "too_many_attachments",
        ] {
            assert!(is_hopeless(e), "{e} は再送すべきでない");
        }
    }

    /// 列挙に無いエラーは再送すること。
    ///
    /// blocks が拒否されたときに返るエラーは分からないので、直らないと
    /// 分かっているものだけを除いて退避する。`internal_error` は
    /// ドキュメントで transient とされている。
    #[test]
    fn unknown_and_transient_errors_are_retried() {
        for e in [
            "invalid_blocks",
            "invalid_blocks_format",
            "msg_blocks_too_long",
            "invalid_arguments",
            "internal_error",
            "fatal_error",
            "request_timeout",
            "service_unavailable",
            "attachment_payload_limit_exceeded",
            "markdown_text_conflict",
            "some_error_slack_has_not_documented_yet",
        ] {
            assert!(!is_hopeless(e), "{e} は再送すべき");
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

    /// blocks を持つときだけ退避すること。
    ///
    /// blocks が無い payload は外しても変わらないので、送り直しても同じ
    /// エラーで失敗する。
    #[test]
    fn falling_back_needs_blocks() {
        let with_blocks = payload(blocks("## body", None));

        assert!(should_fall_back(&with_blocks, "invalid_blocks"));
        assert!(
            should_fall_back(&with_blocks, "internal_error"),
            "拒否されたのに退避していない"
        );
        assert!(
            !should_fall_back(&with_blocks, "invalid_auth"),
            "直らないエラーで退避している"
        );

        for body in [
            Body::new(vec![], Some("body".to_string())),
            Body::new(vec![], None),
        ] {
            assert!(
                !should_fall_back(&payload(body), "invalid_blocks"),
                "blocks が無いのに退避している"
            );
        }
    }

    /// 同じ payload を送り直してよいエラーだけリトライすること。
    ///
    /// `internal_error` と `fatal_error` は「一部が既に成功している可能性が
    /// ある」とドキュメントにあるので、送り直すと通知が重複する。
    #[test]
    fn only_safe_errors_are_retried() {
        assert!(
            is_retriable("service_unavailable"),
            "処理前に断られているので送り直せる"
        );

        for e in [
            // 一部が投稿済みの可能性がある。送り直すと重複する
            "internal_error",
            "fatal_error",
            // 送った内容の不備。同じものを送っても直らない
            "request_timeout",
            "invalid_arguments",
            // 宛先・権限・流量
            "invalid_auth",
            "channel_not_found",
            "ratelimited",
        ] {
            assert!(!is_retriable(e), "{e} は送り直すべきでない");
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

    /// `Content-Type` に charset を付けること。
    ///
    /// 無いと Slack が `missing_charset` を warning で返す。reqwest の
    /// `json()` は charset を付けないので、こちらで先に入れている。
    /// 順序を戻すと `json()` の既定に負けるので、実際に送った値で確かめる。
    #[actix_web::test]
    async fn post_sends_the_charset() {
        let (base, _got, ctypes) = spawn_slack(vec![]);

        message(blocks("## body", Some("body")))
            .post_message_to(&base, "token", "channel", None)
            .await;

        let ctypes = ctypes.lock().unwrap();
        assert_eq!(ctypes.len(), 1, "送信回数が違う");
        assert!(
            ctypes[0].contains("charset=utf-8"),
            "charset が付いていない: {}",
            ctypes[0]
        );
    }

    /// 通ったら 1 回で終わること。
    #[actix_web::test]
    async fn success_posts_once() {
        let (base, got, _ctypes) = spawn_slack(vec![]);

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

    /// Slack の警告を取りこぼさないこと。
    ///
    /// `warning` と `response_metadata.warnings` の両方に返ることがあり、
    /// 同じ内容が重複する。
    #[test]
    fn warnings_are_merged_and_deduped() {
        let res: PostResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "warning": "missing_charset,superfluous_charset",
            "response_metadata": { "warnings": ["missing_charset"] }
        }))
        .expect("deserialize できない");

        assert_eq!(res.warnings(), ["missing_charset", "superfluous_charset"]);
    }

    /// 警告が無い応答も読めること。
    ///
    /// `warning` を必須にすると、正常な応答で deserialize が落ちて
    /// 投稿できたのに失敗扱いになる。
    #[test]
    fn response_without_warnings_is_accepted() {
        let res: PostResponse = serde_json::from_value(serde_json::json!({ "ok": true }))
            .expect("deserialize できない");

        assert!(res.ok);
        assert!(res.warnings().is_empty(), "{:?}", res.warnings());
    }

    /// blocks 由来のエラーなら、従来の表現で再送すること。
    #[actix_web::test]
    async fn block_error_is_retried_as_text() {
        let (base, got, _ctypes) = spawn_slack(rejected("invalid_blocks"));

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

    /// 拒否されたら退避して送り直すこと。
    ///
    /// 単体テストだけでは
    /// 「1 回目の応答を読んで 2 回目を投げる」という手順自体が壊れても
    /// 気付けないので、HTTP を通して確かめる。
    #[actix_web::test]
    async fn transient_error_is_retried_as_text() {
        let (base, got, _ctypes) = spawn_slack(rejected("internal_error"));

        message(blocks("## body", Some("*Assignees*: sksat")))
            .post_message_to(&base, "token", "channel", None)
            .await;

        let got = got.lock().unwrap();
        assert_eq!(got.len(), 2, "再送していない");

        let a = &got[1]["attachments"][0];
        assert_eq!(a["text"], "*Assignees*: sksat", "退避先になっていない: {a}");
        assert!(a.get("blocks").is_none(), "blocks が残っている: {a}");
    }

    /// 本文が無い payload でも、一時的なエラーなら再送すること。
    ///
    /// 退避しても payload は変わらないが、一時的な失敗なら同じものを
    /// 送り直して通る。ここを落とすと本文の無い通知が消える。
    #[actix_web::test]
    async fn blockless_payloads_are_retried_on_transient_errors() {
        let (base, got, _ctypes) = spawn_slack(rejected("service_unavailable"));

        message(Body::new(vec![], None))
            .post_message_to(&base, "token", "channel", None)
            .await;

        assert_eq!(got.lock().unwrap().len(), 2, "再送していない");
    }

    /// 処理前に断られたときは表現を落とさず同じものを送ること。
    ///
    /// 退避を先に判定すると、blocks を持つ payload では `service_unavailable`
    /// でも表現が落ちてしまう。落とす理由が無い。
    #[actix_web::test]
    async fn retriable_errors_keep_the_blocks() {
        let (base, got, _ctypes) = spawn_slack(rejected("service_unavailable"));

        message(blocks("## body", Some("body")))
            .post_message_to(&base, "token", "channel", None)
            .await;

        let got = got.lock().unwrap();
        assert_eq!(got.len(), 2, "送り直していない");
        assert_eq!(got[0], got[1], "表現が落ちている");
    }

    /// blocks 由来でないエラーでは再送しないこと。
    #[actix_web::test]
    async fn other_errors_are_not_retried() {
        let (base, got, _ctypes) = spawn_slack(rejected("invalid_auth"));

        message(blocks("## body", Some("body")))
            .post_message_to(&base, "token", "channel", None)
            .await;

        assert_eq!(got.lock().unwrap().len(), 1, "無駄に再送している");
    }

    /// blocks を持たない payload では再送しないこと。
    #[actix_web::test]
    async fn blockless_payloads_are_not_retried() {
        let (base, got, _ctypes) = spawn_slack(rejected("invalid_blocks"));

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
