use std::time::Duration;

use serde::Serialize;

use tracing::{debug, error};

/// Slack への 1 リクエストのタイムアウト。
///
/// GitHub の webhook 配信タイムアウト (10 秒) を超えると再送されるため、
/// 応答しない Slack を無制限に待たない。
pub const POST_TIMEOUT: Duration = Duration::from_secs(5);

/// markdown ブロックに入れる本文の上限 (**文字数**)。
///
/// Slack の上限は payload 全体で 12,000 文字。GitHub の本文は 65,536 文字まで
/// あるので、超える分は切る。
///
/// バイト数で測ってはいけない。日本語は 1 文字 3 バイトなので、
/// 4,000 文字で 12,000 バイトに達し、Slack の上限より遥かに手前で
/// 切ってしまう。
///
/// Assignees など別のブロックの分を残して、payload 全体の上限より
/// 少なく取ってある。
const MAX_MARKDOWN_CHARS: usize = 11_000;

/// 切り詰めたことを示す印。
const TRUNCATION_MARK: &str = "\n\n_(truncated)_";

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

impl Block {
    /// ブロックの本文。テストで中身を確認するために使う。
    #[cfg(test)]
    pub fn text(&self) -> &str {
        match self {
            Self::Markdown { text } => text,
        }
    }

    /// 本文と、**必ず残したい末尾** (Assignees など) からブロックを作る。
    ///
    /// - 全体が空なら `None`。空の `text` は `invalid_blocks` で拒否される
    ///   (= 通知が飛ばなくなる) ので、ブロックそのものを作らない
    /// - 上限を超える場合は、末尾の分を先に確保して**本文だけ**を切る。
    ///   単純に連結してから切ると、本文が長いときに末尾が消える
    /// - 上限は文字数で数える。バイト数で測ると、日本語は 1 文字 3 バイト
    ///   なので上限の 1/3 の文字数で切られてしまう
    pub fn markdown(body: &str, suffix: &str) -> Option<Self> {
        if body.trim().is_empty() && suffix.trim().is_empty() {
            return None;
        }

        let suffix_len = suffix.chars().count();
        let room = MAX_MARKDOWN_CHARS.saturating_sub(suffix_len);

        let text = if body.chars().count() > room {
            let head: String = body
                .chars()
                .take(room.saturating_sub(TRUNCATION_MARK.chars().count()))
                .collect();
            format!("{head}{TRUNCATION_MARK}{suffix}")
        } else {
            format!("{body}{suffix}")
        };

        Some(Self::Markdown { text })
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
        assert_eq!(
            Block::markdown(md, "").unwrap().text(),
            md,
            "変換せずそのまま渡す"
        );
    }

    #[test]
    fn long_markdown_is_truncated() {
        let md = "a".repeat(MAX_MARKDOWN_CHARS + 100);
        let block = Block::markdown(&md, "").unwrap();

        assert!(
            block.text().chars().count() <= MAX_MARKDOWN_CHARS,
            "上限を超えている"
        );
        assert!(block.text().ends_with("_(truncated)_"), "印が無い");
    }

    /// 上限は文字数で数えること。
    ///
    /// バイト数で測ると、日本語は 1 文字 3 バイトなので上限の 1/3 の
    /// 文字数で切られてしまう。
    #[test]
    fn limit_is_counted_in_characters_not_bytes() {
        // 上限ぴったりの文字数 (バイト数では 3 倍になる)
        let md = "あ".repeat(MAX_MARKDOWN_CHARS);
        let block = Block::markdown(&md, "").unwrap();

        assert_eq!(block.text(), md, "文字数は上限内なので切ってはいけない");
        assert!(
            md.len() > MAX_MARKDOWN_CHARS,
            "テストの前提: バイト数は超える"
        );
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

    /// 末尾は本文が長くても消えないこと。
    ///
    /// 連結してから切ると、本文が上限に達した時点で末尾が失われる。
    #[test]
    fn suffix_survives_truncation() {
        let body = "a".repeat(MAX_MARKDOWN_CHARS * 2);
        let suffix = "\n**Assignees**\nsksat";
        let block = Block::markdown(&body, suffix).unwrap();

        assert!(block.text().ends_with(suffix), "末尾が消えている");
        assert!(
            block.text().chars().count() <= MAX_MARKDOWN_CHARS,
            "上限を超えている"
        );
    }

    /// 本文が無くても末尾だけでブロックを作れること (assigned イベント)。
    #[test]
    fn suffix_only_makes_a_block() {
        let block = Block::markdown("", "**Assignees**\nsksat").unwrap();
        assert_eq!(block.text(), "**Assignees**\nsksat");
    }
}
