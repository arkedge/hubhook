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
    // どちらも octokit の schema では required ではない。
    // organization は個人リポジトリ、installation は GitHub App 以外の
    // webhook で入らないので、必須にすると deserialize が失敗して
    // 通知が止まる (どちらも読んでいないフィールド)。
    pub organization: Option<common::Organization>,
    pub sender: common::User,
    pub installation: Option<common::InstallationLite>,
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
    // どちらも octokit の schema では required ではない。
    // organization は個人リポジトリ、installation は GitHub App 以外の
    // webhook で入らないので、必須にすると deserialize が失敗して
    // 通知が止まる (どちらも読んでいないフィールド)。
    pub organization: Option<common::Organization>,
    pub sender: common::User,
    pub installation: Option<common::InstallationLite>,
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
        // 「本文 + 展開結果」は rule ごとに使うので、ここで 1 回だけ組み立てる。
        // rule ごとに format! すると、展開結果が大きいときに rule 数だけ
        // 確保と走査を繰り返すことになる。
        let combined = if extra_mentions.is_empty() {
            String::new()
        } else {
            format!("{body} {extra_mentions}", body = self.body())
        };

        let mut v = HashMap::<String, RuleMatchResult>::new();

        for r in rules {
            // not match
            if !r.check_match(self, extra_mentions, &combined) {
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

    /// `test/` の payload を deserialize する。
    ///
    /// 失敗したフィールドが分かるように、エラーをそのまま panic に出す。
    pub(crate) fn de(event: &str, test_json: &str) -> Payload {
        let path = format!("test/{test_json}");
        let payload =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {path}: {e}"));

        Payload::from_event(event, payload.as_bytes())
            .unwrap_or_else(|e| panic!("{test_json}: {e}"))
            .expect("unsupported event")
    }

    /// `test/` の payload からトップレベルのフィールドを落として deserialize する。
    ///
    /// examples に無い形を 1 テストのために作るのに fixture を足すと、
    /// 数百行のコピーがリポジトリに残る。
    ///
    /// 落とすフィールドが元の payload に無ければ panic する。上流の
    /// example が変わったときに、テストが意図と違う形を見ないようにする。
    pub(crate) fn de_without(event: &str, test_json: &str, keys: &[&str]) -> Payload {
        let path = format!("test/{test_json}");
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {path}: {e}"));

        let mut value: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{test_json}: {e}"));
        let obj = value.as_object_mut().expect("payload が object ではない");
        for key in keys {
            assert!(obj.remove(*key).is_some(), "{test_json} に {key} が無い");
        }

        let body = serde_json::to_vec(&value).expect("直列化に失敗");
        Payload::from_event(event, &body)
            .unwrap_or_else(|e| panic!("{test_json}: {e}"))
            .expect("unsupported event")
    }
}

#[cfg(test)]
mod tests {
    use crate::github::testing::de;
    use crate::github::*;
    use serde_json::Value;

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
        assert_eq!(p.requested_reviewers(), vec!["octo-team"]);
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
        assert!(p.body().contains("@Octocoders/octo-team"));
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
            "@Octocoders/octo-team",
            "末尾アンカーの検証に使う fixture"
        );

        // include: 末尾アンカーが展開後も効くこと
        let rules = vec![
            serde_json::from_str::<crate::Rule>(
                r#"{"channel":"anchored","display_name":"x","query":{"body":"@Octocoders/octo-team$"}}"#,
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
                r#"{"channel":"excluded","display_name":"x","query":{"body":"octo-team"},"exclude_query":{"body":"@Octocoders/octo-team$"}}"#,
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

    /// #286: 展開された login のマッチが並び順に依存しないこと。
    ///
    /// まとめて 1 つの文字列に当てると、`@sksat$` は sksat が
    /// たまたま最後に並んだときだけ一致してしまう。
    #[test]
    fn expanded_member_matching_is_order_independent() {
        let p = de(
            "pull_request_review",
            "pull_request_review.team_mention.derived.json",
        );

        let rules = vec![
            serde_json::from_str::<crate::Rule>(
                r#"{"channel":"anchored-member","display_name":"x","query":{"body":"@sksat$"}}"#,
            )
            .unwrap(),
        ];

        // 最後に並んでいる場合
        assert!(
            !p.match_rules(&rules, "@aaa @sksat").is_empty(),
            "末尾にいるときはマッチするべき"
        );

        // 途中に並んでいる場合も同じ結果になること
        assert!(
            !p.match_rules(&rules, "@aaa @sksat @zzz").is_empty(),
            "並び順で結果が変わっている"
        );
    }

    /// #286 の既知の制限: 「本文の文脈 + member への末尾アンカー」は
    /// 展開結果の並び順に依存する。
    ///
    /// 本文を member ごとに連結して照合すれば解消するが、rule ごと ×
    /// member ごとに本文長を走査することになり、rule が増えるほど webhook
    /// 1 通の処理が重くなる。稀な書き方なので制限として残している。
    /// 解消する場合は、走査量の上限を webhook 単位で設計する必要がある。
    #[test]
    fn known_limitation_contextual_anchor_depends_on_member_order() {
        let p = de(
            "pull_request_review",
            "pull_request_review.team_mention.derived.json",
        );
        assert!(p.body().contains("レビュー"));

        let rules = vec![
            serde_json::from_str::<crate::Rule>(
                r#"{"channel":"ctx","display_name":"x","query":{"body":"レビュー.*@sksat$"}}"#,
            )
            .unwrap(),
        ];

        // sksat が最後に並んでいる場合
        assert!(
            !p.match_rules(&rules, "@aaa @sksat").is_empty(),
            "末尾にいるときはマッチするべき"
        );

        // 後ろに別のメンバーが並ぶとマッチしない (既知の制限)。
        // ここが通るように変えるなら、走査量の上限も併せて設計すること。
        assert!(
            p.match_rules(&rules, "@aaa @sksat @zzz").is_empty(),
            "制限が解消されている。README と このテストの意図を更新すること"
        );
    }

    /// body クエリを使わない rule しか無ければ、team を引く必要がないこと。
    #[test]
    fn rules_without_body_query_do_not_need_expansion() {
        let no_body: crate::Rule = serde_json::from_str(
            r#"{"channel":"c","display_name":"x","query":{"repo":"hubhook","label":"bug"}}"#,
        )
        .unwrap();
        assert!(!no_body.uses_body());

        let with_body: crate::Rule =
            serde_json::from_str(r#"{"channel":"c","display_name":"x","query":{"body":"@sksat"}}"#)
                .unwrap();
        assert!(with_body.uses_body());

        // exclude_query 側だけで使っている場合も展開が必要
        let exclude_body: crate::Rule = serde_json::from_str(
            r#"{"channel":"c","display_name":"x","query":{"repo":"hubhook"},"exclude_query":{"body":"@sksat"}}"#,
        )
        .unwrap();
        assert!(exclude_body.uses_body());
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

    /// **schema の `required` だけで作った payload を deserialize できること。**
    ///
    /// これが payload に対する保証。struct が schema より厳しい (schema で
    /// required でないフィールドを必須にしている) と落ちる。落ちたフィールドは、
    /// その payload が来たときに通知が飛ばなくなる箇所そのもの。
    ///
    /// schema は実行時に取ってくる。fixture を置くと octokit 側が変わるたびに
    /// 追随が必要になるし、置いたまま古くなると保証にならない。取ってくれば
    /// required の追加や action の追加が自動で入る。
    ///
    /// 取得元は 1 ファイルにまとまった bundle
    /// (<https://www.npmjs.com/package/@octokit/webhooks-schemas>)。
    /// `$ref` が内部参照だけになるので、ファイル間のパス解決が要らない。
    #[actix_web::test]
    async fn minimal_required_payloads_deserialize() {
        let schema = octokit_schema().await;
        let defs = schema["definitions"]
            .as_object()
            .expect("definitions が無い");

        let mut per_event = std::collections::HashMap::new();

        for (name, def) in defs {
            // `<event>$<action>` の形をしている
            let Some((event, _action)) = name.split_once('$') else {
                continue;
            };
            if !SUPPORTED_EVENTS.contains(&event) {
                continue;
            }

            for (selected, payload) in enumerate_payloads(def, defs) {
                let body = serde_json::to_vec(&payload).expect("直列化に失敗");

                Payload::from_event(event, &body)
                    .unwrap_or_else(|e| panic!("{name} {selected}: {e}"))
                    .expect("unsupported event");
            }

            *per_event.entry(event).or_insert(0) += 1;
        }

        // 命名が変わって丸ごと filter されても総数だけでは気付けないので、
        // イベントごとに 1 件以上あることを見る
        for event in SUPPORTED_EVENTS {
            let n = per_event.get(event).copied().unwrap_or(0);
            assert!(n > 0, "{event} の definition が 1 つも見つからない");
        }
    }

    /// action enum が schema の action を網羅していること。
    ///
    /// 未知の action は deserialize が落ちて webhook 全体が 400 になり、
    /// 通知が失われる。GitHub が action を追加したらここで気付ける。
    #[actix_web::test]
    async fn every_schema_action_is_known() {
        let schema = octokit_schema().await;
        let defs = schema["definitions"]
            .as_object()
            .expect("definitions が無い");

        let mut missing = Vec::new();
        let mut per_event = std::collections::HashMap::new();

        for name in defs.keys() {
            let Some((event, action)) = name.split_once('$') else {
                continue;
            };
            if !SUPPORTED_EVENTS.contains(&event) {
                continue;
            }

            *per_event.entry(event).or_insert(0) += 1;

            // action だけを持つ payload で、enum が受け付けるかを見る
            let probe = serde_json::json!({ "action": action });
            let accepted = match event {
                "issues" => serde_json::from_value::<ActionOnly<IssuesAction>>(probe).is_ok(),
                "issue_comment" => {
                    serde_json::from_value::<ActionOnly<IssueCommentAction>>(probe).is_ok()
                }
                "pull_request" => {
                    serde_json::from_value::<ActionOnly<PullRequestAction>>(probe).is_ok()
                }
                "pull_request_review" => {
                    serde_json::from_value::<ActionOnly<PullRequestReviewAction>>(probe).is_ok()
                }
                "pull_request_review_comment" => {
                    serde_json::from_value::<ActionOnly<PullRequestReviewCommentAction>>(probe)
                        .is_ok()
                }
                _ => unreachable!("SUPPORTED_EVENTS と一致していない"),
            };

            if !accepted {
                missing.push(name.clone());
            }
        }

        assert!(missing.is_empty(), "enum が知らない action: {missing:?}");

        // 命名が変わって丸ごと filter されても気付けるように
        for event in SUPPORTED_EVENTS {
            let n = per_event.get(event).copied().unwrap_or(0);
            assert!(n > 0, "{event} の definition が 1 つも見つからない");
        }
    }

    /// hubhook が扱うイベント。
    const SUPPORTED_EVENTS: &[&str] = &[
        "issues",
        "issue_comment",
        "pull_request",
        "pull_request_review",
        "pull_request_review_comment",
    ];

    /// `action` だけを取り出して enum を試すための入れ物。
    #[derive(Deserialize)]
    struct ActionOnly<T> {
        #[allow(dead_code)]
        action: T,
    }

    /// schema の中の位置。
    ///
    /// パスは選択肢のある箇所でしか使わないのに、文字列で持つと全プロパティ分
    /// 生成することになる (payload 数 x プロパティ数で数百万回)。親へのリンクだけ
    /// 持って、必要になったときに組み立てる。
    struct Loc<'a> {
        parent: Option<&'a Loc<'a>>,
        /// 親との区切り。`/` は property や配列の要素、`|` は oneOf の枝番
        sep: char,
        seg: &'a str,
    }

    impl Loc<'_> {
        fn root() -> Self {
            Loc {
                parent: None,
                sep: '/',
                seg: "",
            }
        }

        fn child<'a>(&'a self, sep: char, seg: &'a str) -> Loc<'a> {
            Loc {
                parent: Some(self),
                sep,
                seg,
            }
        }

        fn path(&self) -> String {
            let Some(parent) = self.parent else {
                return String::new();
            };

            let mut out = parent.path();
            out.push(self.sep);
            out.push_str(self.seg);

            out
        }
    }

    /// `oneOf` / `anyOf` の枝番。`format!` を避けるために表で持つ。
    ///
    /// hubhook が辿る範囲での最大 branch 数は 3。足りなければ最後のものを使う
    /// (パスが衝突しても、その位置の選択肢が 1 つに潰れるだけ)。
    const BRANCH_SEGS: &[&str] = &["0", "1", "2", "3", "4", "5", "6", "7"];

    /// 選択肢の種類。同じ位置に複数の選択肢が来ることがある
    /// (`type: ["array", "null"]` の配列と、その配列を埋めるかどうかなど)。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    enum Kind {
        Type,
        Enum,
        Branch,
        Items,
    }

    /// 選択肢の位置と種類 -> 選ぶ候補の index。
    type Selections = std::collections::BTreeMap<(String, Kind), usize>;

    /// `at` に到達するのに必要な選択だけを残す。
    ///
    /// 全部引き継ぐと兄弟の選択肢との掛け算になって組み合わせが爆発する。
    /// 逆に何も引き継がないと、外側の選択で初めて現れる内側の選択肢
    /// (空でない配列の要素など) に到達できない。
    fn enabling(selections: &Selections, at: &str) -> Selections {
        selections
            .iter()
            .filter(|((path, _), _)| {
                at == path
                    || at.starts_with(&format!("{path}/"))
                    || at.starts_with(&format!("{path}|"))
            })
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// 1 つの definition から、選択肢を全部通る payload を列挙する。
    ///
    /// schema が選択肢を許す箇所 (型・`oneOf` / `anyOf` の branch・enum の値・
    /// 配列の中身) は、1 つだけ試すと他を受けない型に戻しても気付けない。
    ///
    /// グローバルな index 1 つで回すと**入れ子の選択肢を網羅できない**。
    /// 外側で別の枝を選ぶと内側の選択肢に到達しないので、例えば
    /// `oneOf: [{ enum: [A, B] }, string]` は `B` が一度も生成されない。
    /// そこで選択肢ごとに、そこへ到達できる選び方 + その候補で payload を作る。
    ///
    /// `required` だけの形と optional も入れた形の両方を回す。optional を
    /// 入れないと、そのフィールドの型を間違えていてもキーが来ないので
    /// 気付けない。中の選択肢も同じように列挙する。
    fn enumerate_payloads(
        def: &Value,
        defs: &serde_json::Map<String, Value>,
    ) -> Vec<(String, Value)> {
        // 選択肢が増えると組み合わせが増えるので、暴走したら気付けるようにする
        const MAX_PAYLOADS: usize = 40_000;

        let mut out = Vec::new();
        let mut queue = vec![(false, Selections::new()), (true, Selections::new())];
        let mut tried = std::collections::HashSet::new();

        while let Some((all_props, selections)) = queue.pop() {
            if !tried.insert((all_props, selections.clone())) {
                continue;
            }
            assert!(
                out.len() < MAX_PAYLOADS,
                "payload が多すぎる (選択肢の暴走?)"
            );

            let ctx = Ctx {
                defs,
                selections: &selections,
                all_props,
            };

            let mut found = Selections::new();
            let payload = build(&ctx, def, None, &Loc::root(), &mut found, 0);

            let mut label = if all_props {
                "optional 込み".to_string()
            } else {
                "required だけ".to_string()
            };
            for ((path, kind), k) in &selections {
                label.push_str(&format!(" {path}:{kind:?}=#{k}"));
            }
            out.push((label, payload));

            for ((path, kind), candidates) in found {
                for k in 1..candidates {
                    let mut next = enabling(&selections, &path);
                    next.insert((path.clone(), kind), k);
                    queue.push((all_props, next));
                }
            }
        }

        out
    }

    /// octokit の schema bundle。`target/` に置くので git には入らない。
    ///
    /// **CI では必ず取り直す。手元ではキャッシュを使う。**
    ///
    /// 保証が効いていないと困るのはマージの門である CI なので、そこでは毎回
    /// 取得して octokit 側の変更を必ず拾う。キャッシュを優先すると一度保存した
    /// 後は二度と取得せず、required や action が増えても永久に気付けない
    /// (置き場所が変わっただけの fixture になる)。
    ///
    /// 手元でキャッシュを使うのは、毎回の `cargo test` を速くするためと、
    /// オフラインでも動かせるようにするため。手元が古くても CI が拾う。
    async fn octokit_schema() -> &'static Value {
        // テストは並列に走る。それぞれが取得するとネットワークも
        // キャッシュへの書き込みも競合するので、プロセスで 1 回にまとめる
        static SCHEMA: tokio::sync::OnceCell<Value> = tokio::sync::OnceCell::const_new();

        SCHEMA.get_or_init(fetch_octokit_schema).await
    }

    async fn fetch_octokit_schema() -> Value {
        const URL: &str = "https://cdn.jsdelivr.net/npm/@octokit/webhooks-schemas/schema.json";

        let cache = cache_path();

        if !in_ci()
            && let Ok(cached) = std::fs::read(&cache)
            && let Ok(schema) = serde_json::from_slice(&cached)
        {
            return schema;
        }

        let body = fetch_schema(URL)
            .await
            .unwrap_or_else(|e| panic!("schema を取得できない ({URL}): {e}"));

        let schema = serde_json::from_slice(&body).expect("schema が JSON でない");

        // 手元の次回以降と、オフライン時のために残す。
        // 直接書くと、並列で走る別のテストが書きかけを読んでしまう
        // 名前を共有すると、別に走っている cargo test の書きかけを
        // rename してしまう
        let tmp = cache.with_extension(format!("tmp.{}", std::process::id()));
        let written = std::fs::write(&tmp, &body).and_then(|()| std::fs::rename(&tmp, &cache));

        // 黙って失敗すると、オフラインで動くという前提が崩れたことに気付けない
        if let Err(e) = written {
            eprintln!("警告: キャッシュを書けない ({}): {e}", cache.display());
        }

        schema
    }

    /// schema のキャッシュの置き場所。
    ///
    /// `target/` は git に入らないので都合が良い。ただし `CARGO_TARGET_DIR` で
    /// 外に出している場合は `<manifest>/target` が作られないので、そちらを見る。
    fn cache_path() -> std::path::PathBuf {
        let dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target"));

        // 手元で一度もビルドしていないと無いことがある
        let _ = std::fs::create_dir_all(&dir);

        dir.join("octokit-webhooks-schema.json")
    }

    /// CI で走っているか。GitHub Actions は `CI=true` を入れる。
    fn in_ci() -> bool {
        std::env::var("CI").is_ok_and(|v| v == "true" || v == "1")
    }

    async fn fetch_schema(url: &str) -> Result<Vec<u8>, reqwest::Error> {
        // reqwest にはデフォルトのタイムアウトが無い。CDN が応答しないと
        // テストがぶら下がったままになる
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        let body = client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        Ok(body.to_vec())
    }

    /// `$ref` と `allOf` を辿って、参照している schema を並べる。
    ///
    /// 畳んで 1 枚の Map にするとクローンが大量に発生する (`repository` は
    /// 77 プロパティあり、これを payload ごとに何度も複製することになる)。
    /// 参照を並べるだけにして、必要なキーをそこから探す。
    ///
    /// 後ろにあるものが優先 (自分自身のキーが `$ref` 先を上書きする)。
    fn sources<'a>(
        node: &'a Value,
        defs: &'a serde_json::Map<String, Value>,
        out: &mut Vec<&'a serde_json::Map<String, Value>>,
        depth: usize,
    ) {
        assert!(depth < MAX_DEPTH, "schema が深すぎる ($ref の循環?)");

        let Some(obj) = node.as_object() else {
            return;
        };

        if let Some(r) = obj.get("$ref").and_then(Value::as_str) {
            let name = r
                .strip_prefix("#/definitions/")
                .unwrap_or_else(|| panic!("外部参照は解けない: {r}"));
            let target = defs
                .get(name)
                .unwrap_or_else(|| panic!("参照先が無い: {r}"));

            sources(target, defs, out, depth + 1);
        }

        if let Some(parts) = obj.get("allOf").and_then(Value::as_array) {
            for part in parts {
                sources(part, defs, out, depth + 1);
            }
        }

        out.push(obj);
    }

    /// 並べた schema からキーを探す。後ろ (優先度の高い方) から見る。
    fn lookup<'a>(sources: &[&'a serde_json::Map<String, Value>], key: &str) -> Option<&'a Value> {
        sources.iter().rev().find_map(|s| s.get(key))
    }

    /// 並べた schema の `required` の和集合。
    fn required_keys<'a>(sources: &[&'a serde_json::Map<String, Value>]) -> Vec<&'a str> {
        let mut out = Vec::new();

        for s in sources {
            if let Some(names) = s.get("required").and_then(Value::as_array) {
                for n in names.iter().filter_map(Value::as_str) {
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
        }

        out
    }

    /// 埋めるフィールド。`all_props` なら optional も入れる。
    ///
    /// required だけだと、optional なフィールドの型を間違えていても
    /// そのキーが来ないので気付けない。
    fn keys_to_fill<'a>(
        sources: &[&'a serde_json::Map<String, Value>],
        all_props: bool,
    ) -> Vec<&'a str> {
        let mut out = required_keys(sources);

        if !all_props {
            return out;
        }

        for s in sources {
            if let Some(props) = s.get("properties").and_then(Value::as_object) {
                for key in props.keys() {
                    if !out.contains(&key.as_str()) {
                        out.push(key);
                    }
                }
            }
        }

        out
    }

    /// 並べた schema から `properties` の 1 つを探す。
    fn property<'a>(
        sources: &[&'a serde_json::Map<String, Value>],
        key: &str,
    ) -> Option<&'a Value> {
        sources
            .iter()
            .rev()
            .find_map(|s| s.get("properties")?.as_object()?.get(key))
    }

    /// `properties` を持つか。
    fn has_properties(sources: &[&serde_json::Map<String, Value>]) -> bool {
        sources.iter().any(|s| s.contains_key("properties"))
    }

    /// $ref に循環があっても止まるようにする深さの上限。
    ///
    /// 現在の schema で必要な深さは 10 程度。ここに当たったら循環を疑う。
    const MAX_DEPTH: usize = 64;

    /// `required` のフィールドだけを持つ payload を作る。
    ///
    /// 値そのものに意味は無く、型と有無だけが意味を持つ。
    ///
    /// 選択肢のある箇所では `selections` にその位置と種類の指定があればそれを
    /// 使い、無ければ先頭を使う。通った選択肢と候補数は `found` に記録するので、
    /// 呼び出し側が 1 つずつ差し替えて列挙できる。
    /// 1 つの payload を作る間ずっと変わらないもの。
    struct Ctx<'a> {
        defs: &'a serde_json::Map<String, Value>,
        /// 選択肢のある箇所で使う候補
        selections: &'a Selections,
        /// optional なフィールドも入れるか
        all_props: bool,
    }

    fn build(
        ctx: &Ctx,
        node: &Value,
        name: Option<&str>,
        loc: &Loc,
        found: &mut Selections,
        depth: usize,
    ) -> Value {
        assert!(
            depth < MAX_DEPTH,
            "schema が深すぎる ($ref の循環?): {}",
            loc.path()
        );

        let mut srcs = Vec::new();
        sources(node, ctx.defs, &mut srcs, 0);

        // 候補が複数ある箇所を記録して、どれを使うか決める
        let choose = |kind: Kind, candidates: usize, found: &mut Selections| -> usize {
            // 候補が 1 つなら選ぶ余地が無い。ここが大多数なので、
            // パス文字列の生成も map 引きも省く
            if candidates <= 1 {
                return 0;
            }

            let key = (loc.path(), kind);
            found.insert(key.clone(), candidates);

            ctx.selections
                .get(&key)
                .copied()
                .unwrap_or(0)
                .min(candidates - 1)
        };

        // enum も選択肢。null を含むものがあり、それを試さないと
        // Option を外しても気付けない
        if let Some(vs) = lookup(&srcs, "enum").and_then(Value::as_array) {
            if vs.is_empty() {
                return Value::Null;
            }

            return vs[choose(Kind::Enum, vs.len(), found)].clone();
        }
        if let Some(c) = lookup(&srcs, "const") {
            return c.clone();
        }

        let t = match lookup(&srcs, "type") {
            Some(Value::String(t)) => Some(t.as_str()),
            Some(Value::Array(ts)) => {
                let ts: Vec<&str> = ts.iter().filter_map(Value::as_str).collect();
                let k = choose(Kind::Type, ts.len(), found);

                ts.get(k).copied()
            }
            _ => None,
        };

        // 型が決まっていない oneOf / anyOf は branch を選ぶ。
        // branch の中にも選択肢がありうるので、パスに枝番を足して区別する
        if t.is_none() && !has_properties(&srcs) {
            for key in ["oneOf", "anyOf"] {
                if let Some(branches) = lookup(&srcs, key).and_then(Value::as_array) {
                    if branches.is_empty() {
                        return Value::Null;
                    }

                    let k = choose(Kind::Branch, branches.len(), found);
                    let seg = BRANCH_SEGS[k.min(BRANCH_SEGS.len() - 1)];

                    return build(
                        ctx,
                        &branches[k],
                        name,
                        &loc.child('|', seg),
                        found,
                        depth + 1,
                    );
                }
            }
        }

        match t {
            Some("object") | None if has_properties(&srcs) || t.is_some() => {
                let mut out = serde_json::Map::new();

                for key in keys_to_fill(&srcs, ctx.all_props) {
                    let value = match property(&srcs, key) {
                        Some(spec) => {
                            build(ctx, spec, Some(key), &loc.child('/', key), found, depth + 1)
                        }
                        // required なのに properties に無いなら空オブジェクト
                        None => Value::Object(serde_json::Map::new()),
                    };
                    out.insert(key.to_string(), value);
                }

                Value::Object(out)
            }
            Some("array") => {
                // 空だけだと要素の型が一切検証されない。要素側が schema より
                // 厳しくても、中身のある payload が来て初めて落ちることになる
                let items = lookup(&srcs, "items");
                let candidates = if items.is_some() { 2 } else { 1 };

                if choose(Kind::Items, candidates, found) == 0 {
                    return Value::Array(vec![]);
                }

                let items = items.expect("候補が 2 なら items がある");

                Value::Array(vec![build(
                    ctx,
                    items,
                    name,
                    &loc.child('/', "0"),
                    found,
                    depth + 1,
                )])
            }
            Some("boolean") => Value::Bool(false),
            Some("integer") | Some("number") => Value::from(0),
            Some("null") => Value::Null,
            _ => {
                // 文字列。url::Url で受けるフィールドは parse できる形にする
                // (schema が format: uri を付けていないものがある)
                let url_ish =
                    name.is_some_and(|n| n == "url" || n == "href" || n.ends_with("_url"));
                let fmt = lookup(&srcs, "format").and_then(Value::as_str);

                if fmt == Some("uri") || url_ish {
                    Value::from("https://example.com/minimal")
                } else if fmt == Some("date-time") {
                    Value::from("2026-01-01T00:00:00Z")
                } else {
                    Value::from("minimal")
                }
            }
        }
    }
}
