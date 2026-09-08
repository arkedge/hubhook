use std::time::Duration;

use serde::Serialize;

use tracing::{debug, error};

/// Slack への 1 リクエストのタイムアウト。
///
/// GitHub の webhook 配信タイムアウト (10 秒) を超えると再送されるため、
/// 応答しない Slack を無制限に待たない。
pub const POST_TIMEOUT: Duration = Duration::from_secs(5);

/// markdown ブロックに入れる本文の上限。
///
/// Slack の上限は payload 全体で 12,000 文字。GitHub の本文は 65,536 文字まで
/// あるので、超える分は切る。切ったことが分かるように印を付ける。
const MAX_MARKDOWN_LEN: usize = 10_000;

#[derive(Debug)]
pub struct Message {
    pub text: String,
    pub attachments: Option<Vec<Attachment>>,
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
    /// 本文。markdown ブロックとして入れる。
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

impl Block {
    /// ブロックの本文。テストで中身を確認するために使う。
    #[cfg(test)]
    pub fn text(&self) -> &str {
        match self {
            Self::Markdown { text } => text,
        }
    }

    /// Markdown の本文からブロックを作る。長すぎる場合は切る。
    pub fn markdown(text: &str) -> Self {
        let text = if text.len() > MAX_MARKDOWN_LEN {
            // UTF-8 の途中で切らない
            let mut end = MAX_MARKDOWN_LEN;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}\n\n_(truncated)_", &text[..end])
        } else {
            text.to_string()
        };

        Self::Markdown { text }
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
        let payload = MessagePayload {
            channel: channel.to_string(),
            username: username.map(|u| u.to_string()),
            text: self.text,
            fallback: None,
            attachments: self.attachments,
        };

        // post
        //
        // reqwest にはデフォルトのタイムアウトが無い。Slack が応答しないと
        // webhook のレスポンスを返せず、GitHub 側が再送して通知が重複する。
        let client = reqwest::Client::builder()
            .timeout(POST_TIMEOUT)
            .build()
            .expect("could not build http client");
        let r = client
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await;

        debug!("{:?}", &r);

        if r.is_err() {
            error!("POST: {:?}", r.err().unwrap());
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

    #[test]
    fn short_markdown_is_passed_through() {
        let md = "## 概要\n\n**重要** な `code` と [link](https://example.com)";
        assert_eq!(Block::markdown(md).text(), md, "変換せずそのまま渡す");
    }

    #[test]
    fn long_markdown_is_truncated() {
        let md = "a".repeat(MAX_MARKDOWN_LEN + 100);
        let block = Block::markdown(&md);

        assert!(block.text().len() <= MAX_MARKDOWN_LEN + 32, "切れていない");
        assert!(block.text().ends_with("_(truncated)_"), "印が無い");
    }

    /// UTF-8 の途中で切らないこと。
    ///
    /// 上限をバイト数で見ているので、マルチバイト文字の境界に当たると
    /// そのまま切ると panic する。
    #[test]
    fn truncation_respects_char_boundaries() {
        // 3 バイト文字で埋めて、上限が文字境界に来ないようにする
        let md = "あ".repeat(MAX_MARKDOWN_LEN);
        let block = Block::markdown(&md);
        assert!(block.text().ends_with("_(truncated)_"));
    }
}
