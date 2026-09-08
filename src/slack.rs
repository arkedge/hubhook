use std::time::Duration;

use serde::{Deserialize, Serialize};

use tracing::{debug, error, warn};

/// Slack への 1 リクエストのタイムアウト。
///
/// GitHub の webhook 配信タイムアウト (10 秒) を超えると再送されるため、
/// 応答しない Slack を無制限に待たない。
pub const POST_TIMEOUT: Duration = Duration::from_secs(5);

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
/// `invalid_auth` や `channel_not_found` は blocks を外しても直らないので、
/// 再送しても 2 回目が無駄に失敗し、レート制限を悪化させるだけ。
/// ここに無いエラーが blocks 由来だった場合はログに残るので、後から足せる。
fn is_blocks_problem(error: &str) -> bool {
    matches!(
        error,
        "invalid_blocks" | "invalid_blocks_format" | "invalid_arguments" | "msg_too_long"
    )
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
    token: &str,
    payload: &MessagePayload,
) -> Result<(), PostError> {
    let res = client
        .post("https://slack.com/api/chat.postMessage")
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
}

#[derive(Debug, Serialize)]
pub struct Attachment {
    pub title: Option<String>,
    pub title_link: Option<url::Url>,
    pub fallback: String,
    pub color: Option<Color>,
    /// 本文の退避先。markdown ブロックが拒否されたときだけ使う。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 本文。markdown ブロックとして入れる。
    ///
    /// 本文が無いときは空にする。空の `text` を持つブロックを送ると
    /// `invalid_blocks` で拒否され、通知が飛ばなくなる。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ///
    /// attachment の `text` は mrkdwn (Slack 独自記法) なので、GitHub の本文を
    /// そのまま貼ると崩れる (`##` がそのまま出る、`*x*` の強調が入れ替わる)。
    /// markdown ブロックは **本物の Markdown** を解釈するので、見出しや表、
    /// タスクリストまでそのまま渡せる。
    /// 色バーを残したいので、トップレベルではなく attachment の中に置く。
    pub blocks: Vec<Block>,
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
    /// blocks をやめて、本文を attachment の `text` に戻した payload。
    ///
    /// markdown ブロックが受け付けられない場合の退避先。従来の表現なので、
    /// 長い本文は Slack 側で畳まれる。
    fn into_text_fallback(mut self) -> Self {
        for a in self.attachments.iter_mut().flatten() {
            if a.blocks.is_empty() {
                continue;
            }

            a.text = Some(
                a.blocks
                    .iter()
                    .map(Block::text)
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            a.blocks.clear();
        }

        self
    }
}

impl Block {
    /// ブロックの本文。
    pub fn text(&self) -> &str {
        match self {
            Self::Markdown { text } => text,
        }
    }

    /// 本文と末尾 (Assignees など) からブロックを作る。
    ///
    /// 全体が空なら `None`。空の `text` は `invalid_blocks` で拒否され、
    /// 通知そのものが飛ばなくなる。
    ///
    /// 長さは切らない。markdown ブロックには payload 全体で 12,000 文字の
    /// 上限があるが、超えた場合は Slack に拒否させて `text` へ退避する
    /// ([`MessagePayload::into_text_fallback`])。退避先では従来どおり
    /// Slack が長い本文を「Show more」で畳む。
    pub fn markdown(body: &str, suffix: &str) -> Option<Self> {
        if body.trim().is_empty() && suffix.trim().is_empty() {
            return None;
        }

        Some(Self::Markdown {
            text: format!("{body}{suffix}"),
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
        // reqwest にはデフォルトのタイムアウトが無い。Slack が応答しないと
        // webhook のレスポンスを返せず、GitHub 側が再送して通知が重複する。
        let client = reqwest::Client::builder()
            .timeout(POST_TIMEOUT)
            .build()
            .expect("could not build http client");

        let payload = MessagePayload {
            channel: channel.to_string(),
            username: username.map(|u| u.to_string()),
            text: self.text,
            fallback: None,
            attachments: self.attachments,
        };

        match post(&client, token, &payload).await {
            Ok(()) => return,
            // リクエスト自体の失敗は payload を変えても直らない。
            // 再送すると待ち時間も倍になるので諦める。
            Err(PostError::Request(e)) => {
                error!("POST: {e}");
                return;
            }
            Err(PostError::Api(e)) => {
                if !is_blocks_problem(&e) {
                    error!("POST: {e}");
                    return;
                }

                // markdown ブロックが attachment 内で使えるか、本文が上限を
                // 超えたかはこちらで判定できない。blocks 由来と思われる
                // エラーなら、従来の表現 (attachment の text) で再送する。
                warn!("POST rejected ({e}); retrying without markdown blocks");
            }
        }

        let fallback = payload.into_text_fallback();
        if let Err(e) = post(&client, token, &fallback).await {
            error!("POST (fallback): {e}");
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

    fn attachment(blocks: Vec<Block>) -> Attachment {
        Attachment {
            title: None,
            title_link: None,
            fallback: "fallback".to_string(),
            color: None,
            text: None,
            blocks,
        }
    }

    #[test]
    fn short_markdown_is_passed_through() {
        let md = "## 概要\n\n**重要** な `code` と [link](https://example.com)";
        assert_eq!(
            Block::markdown(md, "").unwrap().text(),
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
        assert_eq!(Block::markdown(&md, "").unwrap().text(), md);
    }

    /// 空の本文ではブロックを作らないこと。
    ///
    /// 空の `text` を持つブロックを送ると `invalid_blocks` で拒否され、
    /// 通知そのものが飛ばなくなる。
    #[test]
    fn empty_body_makes_no_block() {
        assert!(Block::markdown("", "").is_none());
        assert!(Block::markdown("   \n  ", "").is_none());
    }

    /// 本文が無くても末尾だけでブロックを作れること (assigned イベント)。
    #[test]
    fn suffix_only_makes_a_block() {
        let block = Block::markdown("", "**Assignees**: sksat").unwrap();
        assert_eq!(block.text(), "**Assignees**: sksat");
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
            "invalid_arguments",
            "msg_too_long",
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

    /// 退避すると、ブロックの本文が attachment の text に移ること。
    #[test]
    fn fallback_moves_blocks_into_text() {
        let payload = MessagePayload {
            channel: "c".to_string(),
            username: None,
            text: "summary".to_string(),
            fallback: None,
            attachments: Some(vec![attachment(vec![
                Block::markdown("## body", "").unwrap(),
            ])]),
        };

        let payload = payload.into_text_fallback();
        let a = &payload.attachments.as_ref().unwrap()[0];

        assert_eq!(a.text.as_deref(), Some("## body"));
        assert!(a.blocks.is_empty(), "blocks が残っている");
    }

    /// ブロックが無い attachment は退避しても変わらないこと
    /// (本文なしのイベントで text を空文字にしないため)。
    #[test]
    fn fallback_leaves_blockless_attachments_alone() {
        let payload = MessagePayload {
            channel: "c".to_string(),
            username: None,
            text: "summary".to_string(),
            fallback: None,
            attachments: Some(vec![attachment(vec![])]),
        };

        let payload = payload.into_text_fallback();
        let a = &payload.attachments.as_ref().unwrap()[0];

        assert!(a.text.is_none(), "text が付いている");
    }
}
