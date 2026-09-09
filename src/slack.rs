use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use tracing::{debug, error, info, warn};

/// 同じ payload を送り直す回数の上限。
///
/// 予算だけで打ち切ると、応答が速い相手に対して使い切るまで投げ続けてしまう。
///
/// 退避 (payload を変える) はこの上限に数えない。退避すると blocks が消えて
/// 2 度目は成立しないので高々 1 回で、作った退避先を送らずに終わるのを
/// 避けたい。
const MAX_RETRIES: usize = 2;

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

/// 同じ payload で再送すべきか。
///
/// [`is_hopeless`] に無いエラーは一時的なものの可能性があるので送り直す。
/// `internal_error` はドキュメントで
/// "The server could not complete your operation(s) without encountering an
/// error, likely due to a transient issue on our end." とされている。
///
/// 一覧から漏れたエラーで通知が無言で落ちるより、無駄に 1 回投げる方が
/// 損が小さい。再送は [`POST_BUDGET`] の中で行う。
fn should_retry(error: &str) -> bool {
    !is_hopeless(error)
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
    /// attachment の `text` として送る。中身は mrkdwn。
    Text { text: String },
    /// 本文が無い。
    ///
    /// 空文字を送ると `no_text` で拒否され、通知そのものが飛ばなくなるので、
    /// 空なら何も入れない。
    Empty {},
}

impl Body {
    pub fn new(text: Option<String>) -> Self {
        match text {
            Some(text) => Self::Text { text },
            None => Self::Empty {},
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
}

impl MessagePayload {
    /// 本文をどの表現で送ったか。ログに出す。
    fn body_kind(&self) -> &'static str {
        if self
            .attachments
            .iter()
            .flatten()
            .any(|a| matches!(a.body, Body::Text { .. }))
        {
            "attachment text"
        } else {
            "no body"
        }
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

    /// `link` は元になった GitHub の item。落ちた通知を後から辿るのに要る。
    pub async fn post_message(
        self,
        token: &str,
        channel: &str,
        username: Option<&str>,
        link: &str,
    ) {
        self.post_message_to(API_BASE, token, channel, username, link)
            .await
    }

    async fn post_message_to(
        self,
        base: &str,
        token: &str,
        channel: &str,
        username: Option<&str>,
        link: &str,
    ) {
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

        // 1 通目が失敗したときの手は 2 つある。
        //
        // - リトライ: 同じものを送る。処理前に断られただけのとき
        // - 退避: ブロックを外して送る。ブロックが拒否されたとき
        //
        // 手を 1 つ選んで終わりにすると「断られた後に送り直したらブロックを
        // 拒否された」のような組み合わせを取りこぼす。手がある限り続ける。
        //
        // 終わるのは、通ったとき / 打つ手が無いとき / 予算が尽きたとき /
        // 同じものを送り直しすぎたとき。退避は blocks を消すので高々 1 回しか
        // 成立せず、ループは必ず止まる。
        let mut payload = payload;
        let mut degraded = false;
        let mut retries = 0;

        loop {
            let Some(left) = remaining(deadline, Instant::now()) else {
                error!(channel, link, "POST gave up: out of budget");
                return;
            }
            // リクエスト自体の失敗は payload を変えても直らない。
            // 再送すると待ち時間も倍になるので諦める。
            Err(PostError::Request(e)) => {
                error!(channel, error = %e, "POST failed");
                return;
            }
            Err(PostError::Api(e)) => {
                if !should_retry(&e) {
                    error!(channel, error = %e, "POST failed");
                    return;
                }

                warn!(channel, error = %e, "POST rejected; retrying");
            }
        };

        // 2 通目も予算の中で送る。取り直すと webhook の締め切りを超えて
        // GitHub が再送し、通知が重複する
        let Some(left) = remaining(deadline, Instant::now()) else {
            error!(channel, "POST retry skipped: out of budget");
            return;
        };

        match post(&client, base, token, &payload, left).await {
            // 1 回目が落ちたこと自体が知りたい情報なので info には落とさない。
            Ok(()) => warn!(channel, body = payload.body_kind(), "POST ok (retry)"),
            Err(e) => error!(channel, error = %e, "POST failed (retry)"),
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

    fn text(body: &str) -> Body {
        Body::new(Some(body.to_string()))
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

    /// 本文は attachment の `text` に入ること。
    ///
    /// attachment の中では `blocks` が一切通らない (`markdown` は
    /// `internal_error`、`rich_text` と `section` は `invalid_attachments`)。
    #[test]
    fn attachment_sends_the_body_as_text() {
        let mrkdwn = "*body*\n\n*Assignees*: <https://github.com/sksat|sksat>";
        let json = serde_json::to_value(payload(text(mrkdwn))).expect("直列化に失敗");
        let a = &json["attachments"][0];

        assert_eq!(a["text"], mrkdwn, "text になっていない: {a}");
        assert!(a.get("blocks").is_none(), "blocks を送っている: {a}");
    }

    /// 本文が無いときは `text` を送らないこと。
    ///
    /// 空文字を送ると `no_text` で拒否され、通知そのものが飛ばなくなる。
    #[test]
    fn bodyless_attachment_sends_no_text() {
        let json = serde_json::to_value(payload(Body::new(None))).expect("直列化に失敗");
        let a = &json["attachments"][0];

        assert!(a.get("text").is_none(), "text が入っている: {a}");
    }

    /// 本文を送ったかどうかをログで言えること。
    #[test]
    fn body_kind_names_the_representation() {
        assert_eq!(payload(text("body")).body_kind(), "attachment text");
        assert_eq!(payload(Body::new(None)).body_kind(), "no body");
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

        let json = serde_json::to_value(payload_with_footer(text("*body*"), Some(footer)))
            .expect("直列化に失敗");
        let a = &json["attachments"][0];

        assert_eq!(a["footer"], "sksat", "{a}");
        assert_eq!(a["footer_icon"], "https://example.com/avatar.png", "{a}");
    }

    /// footer が無いときは何も出ないこと。
    #[test]
    fn no_footer_sends_nothing() {
        let json =
            serde_json::to_value(payload_with_footer(text("*body*"), None)).expect("直列化に失敗");
        let a = &json["attachments"][0];

        assert!(a.get("footer").is_none(), "footer が入っている: {a}");
        assert!(
            a.get("footer_icon").is_none(),
            "footer_icon が入っている: {a}"
        );
    }

    /// blocks を持ち、かつ直らないと分かっていないエラーのときだけ再送すること。
    #[test]
    fn retry_needs_a_fixable_error() {
        assert!(
            should_retry("internal_error"),
            "transient なエラーで再送していない"
        );
        assert!(
            !should_retry("invalid_auth"),
            "直らないエラーで再送している"
        );
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
    fn exhausted_budget_leaves_no_time_for_the_retry() {
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

        message(text("*body*"))
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

        message(text("*body*"))
            .post_message_to(&base, "token", "channel", None)
            .await;

        let got = got.lock().unwrap();
        assert_eq!(got.len(), 1, "余計に送っている");
        assert_eq!(
            got[0]["attachments"][0]["text"], "*body*",
            "text で送っていない: {}",
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

    /// 直るかもしれないエラーなら、同じ payload で再送すること。
    #[actix_web::test]
    async fn retryable_error_resends_the_same_payload() {
        let (base, got, _ctypes) = spawn_slack(rejected("internal_error"));

        message(text("*body*"))
            .post_message_to(&base, "token", "channel", None)
            .await;

        let got = got.lock().unwrap();
        assert_eq!(got.len(), 2, "再送していない");
        assert_eq!(got[0], got[1], "違う payload を送っている");
    }

    /// 直らないと分かっているエラーでは再送しないこと。
    #[actix_web::test]
    async fn other_errors_are_not_retried() {
        let (base, got, _ctypes) = spawn_slack(rejected("invalid_auth"));

        message(text("*body*"))
            .post_message_to(&base, "token", "channel", None)
            .await;

        assert_eq!(got.lock().unwrap().len(), 1, "無駄に再送している");
    }
}
