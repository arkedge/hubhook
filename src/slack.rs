use std::time::Duration;

use serde::Serialize;

use tracing::{debug, error};

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
    pub text: String,
    pub color: Option<Color>,
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
