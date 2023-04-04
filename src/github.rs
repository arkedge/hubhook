pub mod common;

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug)]
pub enum Payload<'a> {
    IssueComment(Box<IssueCommentEvent<'a>>),
    Issues(Box<IssuesEvent<'a>>),
    PullRequest(Box<PullRequestEvent<'a>>),
}

impl IntoMessage for Payload<'_> {
    fn create_message(&self) -> message::CreatedMessage {
        match self {
            Payload::IssueComment(ic) => ic.create_message(),
            Payload::Issues(i) => i.create_message(),
            Payload::PullRequest(p) => p.create_message(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct IssuesEvent<'a> {
    pub action: IssuesAction,
    #[serde(borrow)]
    pub issue: common::Issue<'a>,
    pub repository: common::Repository<'a>,
    pub sender: common::User<'a>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestEvent<'a> {
    pub action: PullRequestAction,
    #[serde(borrow)]
    pub pull_request: common::PullRequest<'a>,
    pub repository: common::Repository<'a>,
    pub sender: common::User<'a>,
}

// Issue Comment & Pull-Request Comment
#[derive(Debug, Deserialize)]
pub struct IssueCommentEvent<'a> {
    pub action: IssueCommentAction,
    #[serde(borrow)]
    pub issue: common::Issue<'a>,
    pub comment: common::IssueComment<'a>,
    pub repository: common::Repository<'a>,
    pub sender: common::User<'a>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
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
#[serde(rename_all = "lowercase")]
pub enum PullRequestAction {
    Assigned,
    AutoMergeDisabled,
    AutoMergeEnabled,
    Closed,
    ConvertedToDraft,
    Edited,
    Labeled,
    Locked,
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
#[serde(rename_all = "lowercase")]
pub enum IssueCommentAction {
    Created,
    Edited,
    Deleted,
}

use crate::{
    message::{self, IntoMessage},
    Rule, RuleMatchResult,
};
impl Payload<'_> {
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
            Payload::Issues(issues) => issues.issue.title,
            Payload::IssueComment(icomment) => icomment.issue.title,
            Payload::PullRequest(pr) => pr.pull_request.title,
        }
    }

    pub fn body(&self) -> &str {
        match &self {
            Payload::Issues(issues) => issues.issue.body.unwrap_or_default(),
            Payload::IssueComment(icomment) => icomment.comment.body,
            Payload::PullRequest(pr) => pr.pull_request.body.unwrap_or_default(),
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
    // use crate::github::*;

    // fn de(test_json: &str) -> Payload {
    //     let path = format!("test/{}", test_json);
    //     let payload = std::fs::read_to_string(path).unwrap();
    //     let p: Payload = serde_json::from_str(&payload).unwrap();
    //     p
    // }

    // TODO: add test for OSS

    //#[test]
    //fn de_issue_comment() {
    //    assert!(matches!(de("issue_comment.json"), Payload::IssueComment(_)));
    //}

    //#[test]
    //fn de_issue() {
    //    assert!(matches!(de("issue_open.json"), Payload::Issues(_)));
    //    assert!(matches!(de("issue_assigned.json"), Payload::Issues(_)));
    //    assert!(matches!(de("issue_labeled.json"), Payload::Issues(_)));
    //}

    //#[test]
    //fn de_pull_request() {
    //    assert!(matches!(
    //        de("pull_request_assign.json"),
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
