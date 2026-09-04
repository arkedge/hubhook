pub mod common;

use std::collections::HashMap;

use serde::Deserialize;

/// `X-GitHub-Event` ヘッダで分岐して deserialize する ([`Payload::from_event`])。
///
/// 以前は `#[serde(untagged)]` だったが、1 フィールドの型不一致で全 variant が不一致になり、
/// どの variant のどのフィールドで失敗したのか分からなかった (#311)。
/// また `issue_comment` の `deleted` / `edited` は `comment` の deserialize が失敗すると
/// `Issues` variant として成立してしまう (action 名が [`IssuesAction`] にも存在し、
/// 必要なフィールドも揃っているため) という誤ルーティングの危険もあった。
#[derive(Debug)]
pub enum Payload {
    IssueComment(Box<IssueComment>),
    Issues(Box<Issues>),
    PullRequest(Box<PullRequest>),
}

/// [`Payload::from_event`] の deserialize 失敗。
#[derive(Debug)]
pub enum DeserializeError {
    /// どのフィールドで失敗したかを持つ
    Field(serde_path_to_error::Error<serde_json::Error>),
    /// JSON の後ろにゴミが付いている
    TrailingData(serde_json::Error),
}

impl std::fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Field(e) => write!(f, "{path}: {inner}", path = e.path(), inner = e.inner()),
            Self::TrailingData(e) => write!(f, "trailing data: {e}"),
        }
    }
}

impl std::error::Error for DeserializeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Field(e) => Some(e.inner()),
            Self::TrailingData(e) => Some(e),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Issues {
    pub action: IssuesAction,
    pub issue: common::Issue,
    pub repository: common::Repository,
    pub organization: common::Organization,
    pub sender: common::User,
    pub installation: common::InstallationLite,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub action: PullRequestAction,
    number: Option<usize>, // あったりなかったりする？
    pub pull_request: common::PullRequest,
    pub repository: common::Repository,
    pub organization: common::Organization,
    pub sender: common::User,
    pub installation: common::InstallationLite,
}

// Issue Comment & Pull-Request Comment
#[derive(Debug, Deserialize)]
pub struct IssueComment {
    pub action: IssueCommentAction,
    pub issue: common::Issue,
    pub comment: common::IssueComment,
    pub repository: common::Repository,
    pub organization: common::Organization,
    pub sender: common::User,
    pub installation: common::InstallationLite,
}

impl IssueComment {
    pub fn is_pull_request(&self) -> bool {
        self.issue.is_pull_request()
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssuesAction {
    Opened,
    Edited,
    Deleted,
    Pinned,
    Unpinned,
    Closed,
    Reopened,
    Assigned,
    Unassigned,
    Labeled,
    Unlabeled,
    Locked,
    Unlocked,
    Transferred,
    Milestoned,
    Demilestoned,
}

// https://docs.github.com/ja/developers/webhooks-and-events/webhooks/webhook-events-and-payloads#pull_request
#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestAction {
    Assigned,
    AutoMergeDisabled,
    AutoMergeEnabled,
    Closed,
    ConvertedToDraft,
    Demilestoned,
    Dequeued,
    Edited,
    Enqueued,
    Labeled,
    Locked,
    Milestoned,
    Opened,
    ReadyForReview,
    Reopened,
    ReviewRequestRemoved,
    ReviewRequested,
    Synchronize,
    Unassigned,
    Unlabeled,
    Unlocked,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCommentAction {
    Created,
    Edited,
    Deleted,
}

use crate::{Rule, RuleMatchResult};
impl Payload {
    /// `X-GitHub-Event` に対応する variant として deserialize する。
    /// 扱わないイベントは `Ok(None)`。
    pub fn from_event(event: &str, body: &[u8]) -> Result<Option<Self>, DeserializeError> {
        fn de<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, DeserializeError> {
            let mut de = serde_json::Deserializer::from_slice(body);
            let payload =
                serde_path_to_error::deserialize(&mut de).map_err(DeserializeError::Field)?;
            // body 全体が JSON であることを確認する (web::Json と同じ挙動)
            de.end().map_err(DeserializeError::TrailingData)?;
            Ok(payload)
        }

        let payload = match event {
            "issues" => Payload::Issues(de(body)?),
            "issue_comment" => Payload::IssueComment(de(body)?),
            "pull_request" => Payload::PullRequest(de(body)?),
            _ => return Ok(None),
        };

        Ok(Some(payload))
    }

    pub fn repo(&self) -> &common::Repository {
        match &self {
            Payload::Issues(issues) => &issues.repository,
            Payload::IssueComment(icomment) => &icomment.repository,
            Payload::PullRequest(pr) => &pr.repository,
        }
    }

    pub fn sender(&self) -> &common::User {
        match &self {
            Payload::Issues(issues) => &issues.sender,
            Payload::IssueComment(icomment) => &icomment.sender,
            Payload::PullRequest(pr) => &pr.sender,
        }
    }

    pub fn title(&self) -> &str {
        match &self {
            Payload::Issues(issues) => &issues.issue.title,
            Payload::IssueComment(icomment) => &icomment.issue.title,
            Payload::PullRequest(pr) => &pr.pull_request.title,
        }
    }

    pub fn body(&self) -> &str {
        match &self {
            Payload::Issues(issues) => {
                if let Some(body) = &issues.issue.body {
                    body
                } else {
                    ""
                }
            }

            Payload::IssueComment(icomment) => &icomment.comment.body,
            Payload::PullRequest(pr) => pr.pull_request.body.as_deref().unwrap_or(""),
        }
    }

    pub fn labels(&self) -> &Vec<common::Label> {
        match &self {
            Payload::Issues(issues) => &issues.issue.labels,
            Payload::IssueComment(icomment) => &icomment.issue.labels,
            Payload::PullRequest(pr) => &pr.pull_request.labels,
        }
    }

    pub fn url(&self) -> &url::Url {
        match &self {
            Payload::Issues(issues) => &issues.issue.url,
            Payload::IssueComment(icomment) => &icomment.comment.url,
            Payload::PullRequest(pr) => &pr.pull_request.url,
        }
    }

    pub fn match_rules(&self, rules: &[Rule]) -> HashMap<String, RuleMatchResult> {
        let mut v = HashMap::<String, RuleMatchResult>::new();

        for r in rules {
            // not match
            if !r.check_match(self) {
                continue;
            }

            // multiple display_name
            let mut display_name = r.display_name.clone();
            if let Some(res) = v.get(&r.channel) {
                display_name = res.display_name.to_string() + "&" + &display_name;
            }

            let res = RuleMatchResult {
                display_name,
                channel: r.channel.clone(),
            };
            v.insert(r.channel.clone(), res);
        }

        v
    }
}

#[cfg(test)]
mod tests {
    use crate::github::*;

    #[allow(dead_code)] // #292 で payload のテストを書くときに使う
    fn de(event: &str, test_json: &str) -> Payload {
        let path = format!("test/{}", test_json);
        let payload = std::fs::read_to_string(path).unwrap();
        Payload::from_event(event, payload.as_bytes())
            .unwrap()
            .expect("unsupported event")
    }

    #[test]
    fn unsupported_event_is_ignored() {
        assert!(Payload::from_event("push", b"{}").unwrap().is_none());
        assert!(Payload::from_event("", b"{}").unwrap().is_none());
    }

    #[test]
    fn deserialize_error_points_at_the_field() {
        // untagged だった頃は
        // "data did not match any variant of untagged enum Payload" しか出なかった
        let body = br#"{"action":"opened","issue":{"url":"not a url"}}"#;
        let err = Payload::from_event("issues", body).unwrap_err();
        assert!(err.to_string().starts_with("issue.url: "), "{err}");
    }

    /// GitHub が送る action 名を全部 deserialize できること。
    /// 一覧は octokit/webhooks の payload-schemas/api.github.com/<event>/ に対応する。
    fn assert_actions<T: for<'de> Deserialize<'de>>(actions: &[&str]) {
        for action in actions {
            let json = format!("\"{action}\"");
            if let Err(e) = serde_json::from_str::<T>(&json) {
                panic!("{action}: {e}");
            }
        }
    }

    #[test]
    fn pull_request_actions() {
        assert_actions::<PullRequestAction>(&[
            "assigned",
            "auto_merge_disabled",
            "auto_merge_enabled",
            "closed",
            "converted_to_draft",
            "demilestoned",
            "dequeued",
            "edited",
            "enqueued",
            "labeled",
            "locked",
            "milestoned",
            "opened",
            "ready_for_review",
            "reopened",
            "review_request_removed",
            "review_requested",
            "synchronize",
            "unassigned",
            "unlabeled",
            "unlocked",
        ]);
    }

    #[test]
    fn issues_actions() {
        assert_actions::<IssuesAction>(&[
            "assigned",
            "closed",
            "deleted",
            "demilestoned",
            "edited",
            "labeled",
            "locked",
            "milestoned",
            "opened",
            "pinned",
            "reopened",
            "transferred",
            "unassigned",
            "unlabeled",
            "unlocked",
            "unpinned",
        ]);
    }

    #[test]
    fn issue_comment_actions() {
        assert_actions::<IssueCommentAction>(&["created", "deleted", "edited"]);
    }

    // TODO: add test for OSS

    //#[test]
    //fn de_issue_comment() {
    //    assert!(matches!(de("issue_comment", "issue_comment.json"), Payload::IssueComment(_)));
    //}

    //#[test]
    //fn de_issue() {
    //    assert!(matches!(de("issues", "issue_open.json"), Payload::Issues(_)));
    //    assert!(matches!(de("issues", "issue_assigned.json"), Payload::Issues(_)));
    //    assert!(matches!(de("issues", "issue_labeled.json"), Payload::Issues(_)));
    //}

    //#[test]
    //fn de_pull_request() {
    //    assert!(matches!(
    //        de("pull_request", "pull_request_assign.json"),
    //        Payload::PullRequest(_)
    //    ));
    //}

    //#[test]
    //fn issues_action() {
    //    assert!(matches!(
    //        serde_json::from_str("\"opened\"").unwrap(),
    //        IssuesAction::Opened
    //    ));
    //    assert!(matches!(
    //        serde_json::from_str("\"closed\"").unwrap(),
    //        IssuesAction::Closed
    //    ));
    //}
}
