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
    PullRequestReview(Box<PullRequestReview>),
    PullRequestReviewComment(Box<PullRequestReviewComment>),
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

// payload schema の写しなので、読んでいないフィールドも残す
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Issues {
    pub action: IssuesAction,
    pub issue: common::Issue,
    pub repository: common::Repository,
    pub organization: common::Organization,
    pub sender: common::User,
    pub installation: common::InstallationLite,
}

// payload schema の写しなので、読んでいないフィールドも残す
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub action: PullRequestAction,
    number: Option<usize>, // あったりなかったりする？
    pub pull_request: common::PullRequest,
    /// `review_requested` / `review_request_removed` で、
    /// user に依頼したときだけ入る (#87)
    pub requested_reviewer: Option<common::User>,
    /// team に依頼したときだけ入る
    pub requested_team: Option<common::Team>,
    pub repository: common::Repository,
    // どちらも octokit の schema では required ではない。
    // organization は個人リポジトリ、installation は GitHub App 以外の
    // webhook で入らないので、必須にすると deserialize が失敗して
    // 通知が止まる (どちらも読んでいないフィールド)。
    pub organization: Option<common::Organization>,
    pub sender: common::User,
    pub installation: Option<common::InstallationLite>,
}

// Issue Comment & Pull-Request Comment
// payload schema の写しなので、読んでいないフィールドも残す
#[allow(dead_code)]
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
    // issue_comment と PR comment の区別に使う想定で残している
    #[allow(dead_code)]
    pub fn is_pull_request(&self) -> bool {
        self.issue.is_pull_request()
    }
}

/// `pull_request_review`: レビューの submit / edit / dismiss。
/// approve 時のメッセージは `review.body` に入る (#285)。
// payload schema の写しなので、読んでいないフィールドも残す
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct PullRequestReview {
    pub action: PullRequestReviewAction,
    pub review: common::Review,
    pub pull_request: common::SimplePullRequest,
    pub repository: common::Repository,
    // organization / installation は org 所有リポジトリ以外では存在しないので Option。
    // 欠けたフィールドで deserialize に失敗すると通知が飛ばなくなる (#311)。
    pub organization: Option<common::Organization>,
    pub sender: common::User,
    pub installation: Option<common::InstallationLite>,
}

/// `pull_request_review_comment`: diff 上のコメントと、それへの返信 (#122)。
// payload schema の写しなので、読んでいないフィールドも残す
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct PullRequestReviewComment {
    pub action: PullRequestReviewCommentAction,
    pub comment: common::ReviewComment,
    pub pull_request: common::SimplePullRequest,
    pub repository: common::Repository,
    pub organization: Option<common::Organization>,
    pub sender: common::User,
    pub installation: Option<common::InstallationLite>,
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

// https://docs.github.com/en/webhooks/webhook-events-and-payloads#pull_request_review
#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestReviewAction {
    Submitted,
    Edited,
    Dismissed,
}

// https://docs.github.com/en/webhooks/webhook-events-and-payloads#pull_request_review_comment
#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestReviewCommentAction {
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
            "pull_request_review" => Payload::PullRequestReview(de(body)?),
            "pull_request_review_comment" => Payload::PullRequestReviewComment(de(body)?),
            _ => return Ok(None),
        };

        Ok(Some(payload))
    }

    pub fn repo(&self) -> &common::Repository {
        match &self {
            Payload::Issues(issues) => &issues.repository,
            Payload::IssueComment(icomment) => &icomment.repository,
            Payload::PullRequest(pr) => &pr.repository,
            Payload::PullRequestReview(review) => &review.repository,
            Payload::PullRequestReviewComment(comment) => &comment.repository,
        }
    }

    pub fn sender(&self) -> &common::User {
        match &self {
            Payload::Issues(issues) => &issues.sender,
            Payload::IssueComment(icomment) => &icomment.sender,
            Payload::PullRequest(pr) => &pr.sender,
            Payload::PullRequestReview(review) => &review.sender,
            Payload::PullRequestReviewComment(comment) => &comment.sender,
        }
    }

    pub fn title(&self) -> &str {
        match &self {
            Payload::Issues(issues) => &issues.issue.title,
            Payload::IssueComment(icomment) => &icomment.issue.title,
            Payload::PullRequest(pr) => &pr.pull_request.title,
            Payload::PullRequestReview(review) => &review.pull_request.title,
            Payload::PullRequestReviewComment(comment) => &comment.pull_request.title,
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

            // approve のメッセージ (#285) と、レビューコメント・その返信 (#122) を
            // body として扱うことで、既存の body クエリでのメンション検出に乗る
            Payload::PullRequestReview(review) => review.review.body.as_deref().unwrap_or(""),
            Payload::PullRequestReviewComment(comment) => &comment.comment.body,
        }
    }

    pub fn labels(&self) -> &Vec<common::Label> {
        match &self {
            Payload::Issues(issues) => &issues.issue.labels,
            Payload::IssueComment(icomment) => &icomment.issue.labels,
            Payload::PullRequest(pr) => &pr.pull_request.labels,
            Payload::PullRequestReview(review) => &review.pull_request.labels,
            Payload::PullRequestReviewComment(comment) => &comment.pull_request.labels,
        }
    }

    pub fn url(&self) -> &url::Url {
        match &self {
            Payload::Issues(issues) => &issues.issue.url,
            Payload::IssueComment(icomment) => &icomment.comment.url,
            Payload::PullRequest(pr) => &pr.pull_request.url,
            // Review には API url が無いので html_url を使う
            Payload::PullRequestReview(review) => &review.review.html_url,
            Payload::PullRequestReviewComment(comment) => &comment.comment.url,
        }
    }

    /// assignee のマッチ対象 (#41)。
    pub fn assignees(&self) -> &[common::User] {
        match &self {
            Payload::Issues(issues) => &issues.issue.assignees,
            Payload::IssueComment(icomment) => &icomment.issue.assignees,
            Payload::PullRequest(pr) => &pr.pull_request.assignees,
            Payload::PullRequestReview(review) => &review.pull_request.assignees,
            Payload::PullRequestReviewComment(comment) => &comment.pull_request.assignees,
        }
    }

    /// review を依頼された相手の名前 (user は login、team は slug) (#87)。
    ///
    /// `pull_request.requested_reviewers` (依頼中の全員) ではなく、
    /// **そのイベントで新たに依頼された相手**だけを返す。全員を返すと、
    /// PR への commit やコメントごとに reviewer 全員へ通知が飛んでしまう。
    pub fn requested_reviewers(&self) -> Vec<&str> {
        let Payload::PullRequest(pr) = self else {
            return Vec::new();
        };
        if pr.action != PullRequestAction::ReviewRequested {
            return Vec::new();
        }

        let mut reviewers = Vec::new();
        if let Some(user) = &pr.requested_reviewer {
            reviewers.push(user.login.as_str());
        }
        if let Some(team) = &pr.requested_team {
            reviewers.push(team.slug.as_str());
        }
        reviewers
    }

    /// `pull_request_review` の review state (`approved` / `changes_requested` /
    /// `commented` など)。それ以外のイベントでは `None`。
    pub fn review_state(&self) -> Option<&str> {
        match &self {
            Payload::PullRequestReview(review) => Some(&review.review.state),
            _ => None,
        }
    }

    /// `extra_mentions` は team メンションを展開した `@login` の列 (#286)。
    ///
    /// 本文と連結せずにそのまま渡す。連結すると、`$` などのアンカーを使う
    /// 既存ルールの意味が変わってしまう (`@org/team$` が末尾に一致しなくなる、
    /// exclude_query 側では除外されるべきものが除外されなくなる)。
    pub fn match_rules(
        &self,
        rules: &[Rule],
        extra_mentions: &str,
    ) -> HashMap<String, RuleMatchResult> {
        let mut v = HashMap::<String, RuleMatchResult>::new();

        for r in rules {
            // not match
            if !r.check_match(self, extra_mentions) {
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

/// `test/` 以下の payload-example を読むテスト用ローダ。
/// message 側のテストからも使う。
#[cfg(test)]
pub(crate) mod testing {
    use super::Payload;

    pub(crate) fn de(event: &str, test_json: &str) -> Payload {
        let path = format!("test/{test_json}");
        let payload =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {path}: {e}"));
        // 失敗したフィールドが分かるように、エラーをそのまま出す
        Payload::from_event(event, payload.as_bytes())
            .unwrap_or_else(|e| panic!("{test_json}: {e}"))
            .expect("unsupported event")
    }
}

#[cfg(test)]
mod tests {
    use crate::github::testing::de;
    use crate::github::*;

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

    /// #122: レビューコメントへの**返信**が deserialize でき、
    /// `in_reply_to_id` と本文が取れること。
    /// 通常のレビューコメントには `in_reply_to_id` のキー自体が無いので、
    /// 返信の payload は別に用意して検証する。
    #[test]
    fn de_review_comment_reply() {
        let p = de(
            "pull_request_review_comment",
            "pull_request_review_comment.reply.derived.json",
        );

        let Payload::PullRequestReviewComment(rc) = &p else {
            panic!("not a review comment: {p:?}");
        };
        assert!(
            rc.comment.in_reply_to_id.is_some(),
            "返信なので in_reply_to_id が入っているべき"
        );
        assert!(p.body().contains("@sksat"), "body = {:?}", p.body());

        // 通常のレビューコメント側は None であること
        let p = de(
            "pull_request_review_comment",
            "pull_request_review_comment.created.with-organization.json",
        );
        let Payload::PullRequestReviewComment(rc) = &p else {
            panic!("not a review comment");
        };
        assert!(rc.comment.in_reply_to_id.is_none());
    }

    /// `review_state` を指定した rule が review 以外のイベントにマッチしないこと。
    ///
    /// query は正規表現なので、以前のように空文字を照合対象にしていると
    /// `.*` や `^$` のようなパターンが review 以外のイベントにもマッチしてしまう。
    #[test]
    fn review_state_query_never_matches_non_review_events() {
        let rule: crate::Rule = serde_json::from_str(
            r#"{"channel":"test","display_name":"x","query":{"review_state":".*"}}"#,
        )
        .unwrap();
        let rules = vec![rule];

        // review イベントには当たる
        let p = de(
            "pull_request_review",
            "pull_request_review.approved.derived.json",
        );
        assert!(
            !p.match_rules(&rules, "").is_empty(),
            "review にはマッチするべき"
        );

        // review_state を持たないイベントには当たらない
        let p = de(
            "pull_request_review_comment",
            "pull_request_review_comment.created.with-organization.json",
        );
        assert!(
            p.match_rules(&rules, "").is_empty(),
            "review_state を持たないイベントにマッチしてはいけない"
        );
    }

    #[test]
    fn pull_request_review_actions() {
        assert_actions::<PullRequestReviewAction>(&["dismissed", "edited", "submitted"]);
    }

    #[test]
    fn pull_request_review_comment_actions() {
        assert_actions::<PullRequestReviewCommentAction>(&["created", "deleted", "edited"]);
    }

    /// octokit/webhooks の payload-examples をそのまま deserialize できること。
    /// フィールドが 1 つ足りないだけで deserialize に失敗し、通知が飛ばなくなるので、
    /// 実物の payload に対して型を検証しておく (#292)。
    #[test]
    fn de_pull_request_review() {
        let p = de("pull_request_review", "pull_request_review.submitted.json");
        assert!(matches!(p, Payload::PullRequestReview(_)));
        // インラインコメントだけを submit した review は body が null
        assert_eq!(p.review_state(), Some("commented"));
        assert_eq!(p.body(), "");

        // 本番は org 所有リポジトリなので organization 入りも確認する
        let p = de(
            "pull_request_review",
            "pull_request_review.submitted.with-organization.json",
        );
        assert_eq!(p.review_state(), Some("commented"));

        let p = de("pull_request_review", "pull_request_review.dismissed.json");
        assert!(matches!(p, Payload::PullRequestReview(_)));
    }

    /// #285: approve に付けたメッセージが body として取れること。
    /// octokit に approved の payload-example が無いため、
    /// submitted.with-organization から state と body だけ差し替えたものを使う。
    #[test]
    fn review_approved_body_is_captured() {
        let p = de(
            "pull_request_review",
            "pull_request_review.approved.derived.json",
        );
        assert_eq!(p.review_state(), Some("approved"));
        assert!(p.body().contains("@sksat"), "body = {:?}", p.body());
    }

    /// #41: assignee の login がマッチ対象として取れること。
    #[test]
    fn assignees_are_exposed() {
        let p = de(
            "pull_request",
            "pull_request.assigned.with-organization.json",
        );
        let logins: Vec<&str> = p.assignees().iter().map(|u| u.login.as_str()).collect();
        assert_eq!(logins, vec!["Codertocat"]);
    }

    /// #87: review を依頼された相手が取れること。
    #[test]
    fn requested_reviewer_is_exposed() {
        let p = de("pull_request", "pull_request.review_requested.json");
        assert_eq!(p.requested_reviewers(), vec!["octocat"]);
    }

    /// organization / installation が無い payload も deserialize できること。
    ///
    /// octokit の `pull_request/assigned` example には `organization` が無い
    /// (個人リポジトリでは付かない)。必須にしていると deserialize が失敗して
    /// 通知が止まるので、実物で確認しておく。
    #[test]
    fn de_pull_request_without_organization() {
        let p = de("pull_request", "pull_request.assigned.json");
        assert!(matches!(p, Payload::PullRequest(_)));

        let logins: Vec<&str> = p.assignees().iter().map(|u| u.login.as_str()).collect();
        assert!(!logins.is_empty(), "assignees が取れていない");
    }

    /// #87: team に review を依頼した場合、slug が取れること。
    ///
    /// schema は `requested_reviewer` か `requested_team` の oneOf で、
    /// team 側の payload-example は octokit に無いので derived を使う。
    #[test]
    fn requested_team_is_exposed() {
        let p = de(
            "pull_request",
            "pull_request.review_requested.team.derived.json",
        );
        assert_eq!(p.requested_reviewers(), vec!["sat-sw"]);
    }

    /// review_requested 以外のイベントでは reviewer は空にする。
    /// ここが空でないと、PR への commit やコメントごとに reviewer 全員へ
    /// 通知が飛んでしまう。
    #[test]
    fn requested_reviewers_are_empty_for_other_events() {
        let p = de(
            "pull_request",
            "pull_request.assigned.with-organization.json",
        );
        assert!(p.requested_reviewers().is_empty());

        // review イベント側の PR にも requested_reviewers は入っているが、
        // 「今依頼された」わけではないので空にする
        let p = de(
            "pull_request_review",
            "pull_request_review.approved.derived.json",
        );
        assert!(p.requested_reviewers().is_empty());
    }

    /// #286: team メンションを展開すると、個人のルールにマッチすること。
    /// 展開前 (extra_mentions が空) ではマッチしないことも確認する。
    #[test]
    fn expanded_team_mention_matches_personal_rule() {
        let rule: crate::Rule = serde_json::from_str(
            r#"{"channel":"test","display_name":"sksat","query":{"body":"@sksat"}}"#,
        )
        .unwrap();
        let rules = vec![rule];

        // body には team メンションだけが書かれている payload
        let p = de(
            "pull_request_review",
            "pull_request_review.team_mention.derived.json",
        );
        assert!(p.body().contains("@arkedge/sat-sw"));
        assert!(!p.body().contains("@sksat"));

        // 展開前: team メンションのままなので個人のルールには当たらない
        assert!(
            p.match_rules(&rules, "").is_empty(),
            "展開前にマッチしてはいけない"
        );

        // 展開後: メンバーの @login が body に足されるのでマッチする
        let matched = p.match_rules(&rules, "@sksat @meltingrabbit");
        assert!(matched.contains_key("test"), "展開後はマッチするべき");
    }

    /// #286: team 展開を足しても、アンカー付きの既存ルールの意味が変わらないこと。
    ///
    /// 本文と展開結果を連結すると `@org/team$` が末尾に一致しなくなり、
    /// exclude_query 側では「除外されるべきものが除外されない」= 余計な通知が飛ぶ。
    #[test]
    fn expansion_does_not_break_anchored_rules() {
        let p = de(
            "pull_request_review",
            "pull_request_review.team_mention_only.derived.json",
        );
        assert_eq!(
            p.body(),
            "@arkedge/sat-sw",
            "末尾アンカーの検証に使う fixture"
        );

        // include: 末尾アンカーが展開後も効くこと
        let rules = vec![
            serde_json::from_str::<crate::Rule>(
                r#"{"channel":"anchored","display_name":"x","query":{"body":"@arkedge/sat-sw$"}}"#,
            )
            .unwrap(),
        ];
        assert!(!p.match_rules(&rules, "").is_empty(), "展開前はマッチする");
        assert!(
            !p.match_rules(&rules, "@sksat @meltingrabbit").is_empty(),
            "展開すると末尾アンカーが効かなくなっている"
        );

        // exclude: 末尾アンカーによる除外が展開後も効くこと
        let rules = vec![
            serde_json::from_str::<crate::Rule>(
                r#"{"channel":"excluded","display_name":"x","query":{"body":"sat-sw"},"exclude_query":{"body":"@arkedge/sat-sw$"}}"#,
            )
            .unwrap(),
        ];
        assert!(p.match_rules(&rules, "").is_empty(), "展開前は除外される");
        assert!(
            p.match_rules(&rules, "@sksat @meltingrabbit").is_empty(),
            "展開すると除外が効かなくなっている"
        );
    }

    /// #286: 本文の文脈と展開された login を組み合わせたパターンが効くこと。
    ///
    /// 展開結果だけに当てると、`レビュー.*@sksat` のようなパターンは
    /// 本文側にも展開側にも一致せず、どこにも当たらなくなる。
    #[test]
    fn expansion_supports_patterns_combining_body_and_member() {
        let p = de(
            "pull_request_review",
            "pull_request_review.team_mention.derived.json",
        );
        assert!(p.body().contains("レビュー"));
        assert!(!p.body().contains("@sksat"));

        let rules = vec![
            serde_json::from_str::<crate::Rule>(
                r#"{"channel":"combined","display_name":"x","query":{"body":"レビュー.*@sksat"}}"#,
            )
            .unwrap(),
        ];

        // 展開前は @sksat が本文に無いのでマッチしない
        assert!(p.match_rules(&rules, "").is_empty(), "展開前はマッチしない");

        // 展開すると、本文の文脈と合わせてマッチする
        assert!(
            !p.match_rules(&rules, "@sksat @meltingrabbit").is_empty(),
            "本文の文脈と展開結果を組み合わせたパターンが効いていない"
        );
    }

    /// #122: レビューコメント (と返信) の本文が body として取れること。
    #[test]
    fn de_pull_request_review_comment() {
        let p = de(
            "pull_request_review_comment",
            "pull_request_review_comment.created.json",
        );
        assert!(matches!(p, Payload::PullRequestReviewComment(_)));
        assert!(!p.body().is_empty(), "body が空");
        // review_state を持たないので review_state クエリにはマッチしない
        assert_eq!(p.review_state(), None);

        let p = de(
            "pull_request_review_comment",
            "pull_request_review_comment.created.with-organization.json",
        );
        assert!(!p.body().is_empty(), "body が空");
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
