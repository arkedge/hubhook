use serde::Serialize;

use tracing::{debug, error};

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

#[allow(unused)]
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
    pub async fn post_message(self, token: &str, channel: &str, username: Option<&str>) {
        let payload = MessagePayload {
            channel: channel.to_string(),
            username: username.map(|u| u.to_string()),
            text: self.text,
            fallback: None,
            attachments: self.attachments,
        };

        // post
        let r = surf::post("https://slack.com/api/chat.postMessage")
            .header(
                surf::http::headers::AUTHORIZATION,
                format!("Bearer {}", &token),
            )
            .body_json(&payload)
            .expect("post json")
            .recv_string()
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
