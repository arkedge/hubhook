// GitHub webhook の payload schema をそのまま写した型定義。
// octokit/webhooks の payload-schemas と突き合わせられることを優先し、
// 現時点で読んでいないフィールドもスキーマの記述として残しているため、
// このモジュールでは dead_code を許可する。
#![allow(dead_code)]

use serde::Deserialize;
use serde::de::IgnoredAny;
use url::Url;

#[derive(Debug, Deserialize)]
pub struct User {
    pub login: String,
    pub id: usize,
    pub node_id: String,
    pub avatar_url: Url,
    pub gravatar_id: String, // if empty, len() == 0
    pub url: Url,
    pub html_url: Url,
    pub followers_url: Url,
    pub following_url: Url,
    pub gists_url: Url,
    pub starred_url: Url,
    pub subscriptions_url: Url,
    pub organizations_url: Url,
    pub repos_url: Url,
    pub events_url: Url,
    pub received_events_url: Url,
    #[serde(rename(deserialize = "type"))]
    pub typ: String,
    pub site_admin: bool,
}

#[derive(Debug, Deserialize)]
pub struct Organization {
    pub login: String,
    pub id: usize,
    pub node_id: String,
    pub url: Url,
    pub repos_url: Url,
    pub events_url: Url,
    pub hooks_url: Url,
    pub issues_url: Url,
    pub members_url: Url,
    pub public_members_url: Url,
    pub avatar_url: Url,
    pub description: Option<String>,
}

/// repository の日時。**unix timestamp と ISO 8601 のどちらでも来る。**
///
/// schema でも `integer | string` になっている
/// (`repository.created_at` / `repository.pushed_at` だけ)。片方に決め打つと、
/// もう片方が来たときに deserialize が落ちて通知が飛ばなくなる。
///
/// 読んでいないフィールドなので、区別できる形にしておくだけでよい。
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Timestamp {
    Unix(i64),
    Iso(String),
}

#[derive(Debug, Deserialize)]
pub struct Repository {
    pub id: usize,
    pub node_id: String,
    pub name: String,
    pub full_name: String,
    pub private: bool,
    pub owner: User,
    pub html_url: Url,
    pub description: Option<String>,
    pub fork: bool,
    pub url: Url,
    pub forks_url: Url,
    pub keys_url: Url,
    pub collaborators_url: Url,
    pub teams_url: Url,
    pub hooks_url: Url,
    pub issue_events_url: Url,
    pub events_url: Url,
    pub assignees_url: Url,
    pub branches_url: Url,
    pub tags_url: Url,
    pub blobs_url: Url,
    pub git_tags_url: Url,
    pub git_refs_url: Url,
    pub trees_url: Url,
    pub statuses_url: Url,
    pub languages_url: Url,
    pub stargazers_url: Url,
    pub contributors_url: Url,
    pub subscribers_url: Url,
    pub subscription_url: Url,
    pub commits_url: Url,
    pub git_commits_url: Url,
    pub comments_url: Url,
    pub issue_comment_url: Url,
    pub contents_url: Url,
    pub compare_url: Url,
    pub merges_url: Url,
    pub archive_url: Url,
    pub downloads_url: Url,
    pub issues_url: Url,
    pub pulls_url: Url,
    pub milestones_url: Url,
    pub notifications_url: Url,
    pub labels_url: Url,
    pub releases_url: Url,
    pub deployments_url: Url,
    pub created_at: Timestamp,
    pub updated_at: String,
    /// 未 push の repo だと null
    pub pushed_at: Option<Timestamp>,
    pub git_url: Url,
    pub ssh_url: String, // "git@github.com:arkedge/hubhook.git"
    pub clone_url: Url,
    pub svn_url: Url,
    pub homepage: Option<String>,
    pub size: usize,
    pub stargazers_count: usize,
    pub watchers_count: usize,
    pub language: Option<String>, // "Dockerfile"
    pub has_issues: bool,
    pub has_projects: bool,
    pub has_downloads: bool,
    pub has_wiki: bool,
    pub has_pages: bool,
    pub forks_count: usize,
    pub mirror_url: Option<Url>,
    pub archived: bool,
    /// schema では required ではない (`allow_forking` と同じ)
    pub disabled: Option<bool>,
    pub open_issues_count: usize,
    pub license: Option<License>,
    // octokit/webhooks の payload-examples には無い。必須にしておくと、
    // GitHub が送ってこない場合に deserialize が失敗して通知が止まるので Option にする
    pub allow_forking: Option<bool>,
    pub is_template: bool,
    pub topics: Vec<String>, // octkit/webhooksになさそう
    pub visibility: String,
    pub forks: usize,
    pub open_issues: usize,
    pub watchers: usize,
    pub default_branch: String,
}

#[derive(Debug, Deserialize)]
pub struct License {
    pub key: String,
    pub name: String,
    pub spdx_id: String,
    pub url: Option<Url>,
    pub node_id: String,
}

#[derive(Debug, Deserialize)]
pub struct Issue {
    pub url: Url,
    pub repository_url: Url,
    pub labels_url: Url,
    pub comments_url: Url,
    pub events_url: Url,
    pub html_url: Url,
    pub id: usize,
    pub node_id: String,
    pub number: usize,
    pub title: String,
    pub user: User,
    /// schema では required ではない。無ければラベル無しとして扱う
    #[serde(default)]
    pub labels: Vec<Label>,
    // 以下はどちらも schema で required ではない。読んでいないので、
    // 既定値を捏造せず「入っていない」をそのまま表す
    pub state: Option<String>,
    pub locked: Option<bool>,
    pub assignee: Option<User>,
    pub assignees: Vec<User>,
    pub milestone: Option<IgnoredAny>,
    pub comments: usize,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub author_association: String,
    pub active_lock_reason: Option<IgnoredAny>,
    pub body: Option<String>,
    pub reactions: Reactions,
    /// schema では required ではない
    pub timeline_url: Option<Url>,
    pub performed_via_github_app: Option<IgnoredAny>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub url: Url,
    pub id: usize,
    pub node_id: String,
    pub html_url: Url,
    pub diff_url: Url,
    pub patch_url: Url,
    pub issue_url: Url,
    pub number: usize, // PR number
    pub state: String,
    pub locked: bool,
    pub title: String,
    pub user: User,
    pub body: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub merged_at: Option<String>,
    pub merge_commit_sha: Option<String>,
    pub assignee: Option<User>, // Issueと挙動が違う？
    pub assignees: Vec<User>,
    pub requested_reviewers: Vec<Reviewer>,
    pub requested_teams: Vec<Team>,
    pub labels: Vec<Label>,
    pub milestone: Option<IgnoredAny>,
    pub draft: bool,
    pub commits_url: Url,
    pub review_comments_url: Url,
    pub review_comment_url: Url,
    pub comments_url: Url,
    pub statuses_url: Url,
    pub head: PullRequestHead,
    pub base: PullRequestBase,
    pub _links: PullRequestLinks,
    pub author_association: String,
    pub auto_merge: Option<IgnoredAny>,
    pub active_lock_reason: Option<IgnoredAny>,
    pub merged: Option<bool>,    // nullになりようがなくない？？？
    pub mergeable: Option<bool>, // ref: https://github.com/octokit/webhooks/blob/ce6ab8f2ca6c8358a415448f71e20d1d50d458f8/payload-schemas/api.github.com/common/pull-request.schema.json#L168-L170
    pub rebaseable: Option<bool>,
    pub mergeable_state: String,
    pub merged_by: Option<User>,
    pub comments: usize,
    pub review_comments: usize,
    pub maintainer_can_modify: bool,
    pub commits: usize,
    pub additions: usize,
    pub deletions: usize,
    pub changed_files: usize,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestHead {
    pub label: String,
    #[serde(rename = "ref")]
    pub ref_: String,
    pub sha: String,
    pub user: User,
    pub repo: Option<Repository>, // fork が消えていると null になる
}

#[derive(Debug, Deserialize)]
pub struct PullRequestBase {
    pub label: String,
    #[serde(rename = "ref")]
    pub ref_: String,
    pub sha: String,
    pub user: User,
    pub repo: Repository,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestLinks {
    // TODO
}

#[derive(Debug, Deserialize)]
pub struct IssueComment {
    pub url: Url,
    pub html_url: Url,
    pub issue_url: Url,
    pub id: usize,
    pub node_id: String,
    pub user: User,
    pub created_at: String,
    pub updated_at: String,
    pub author_association: String,
    pub body: String,
    pub reactions: Reactions,
    pub performed_via_github_app: Option<IgnoredAny>,
}

/// `pull_request_review` / `pull_request_review_comment` に埋め込まれている PR オブジェクト。
///
/// `pull_request` イベントの [`PullRequest`] とは別物で、`mergeable_state` や `commits`、
/// `additions` などを持たない (octokit の payload-schemas で言う `simple_pull_request`)。
/// [`PullRequest`] を使い回すと必須フィールドが足りず deserialize に失敗し、
/// 通知が飛ばなくなるため型を分けている。
#[derive(Debug, Deserialize)]
pub struct SimplePullRequest {
    pub url: Url,
    pub id: usize,
    pub node_id: String,
    pub html_url: Url,
    pub diff_url: Url,
    pub patch_url: Url,
    pub issue_url: Url,
    pub number: usize,
    pub state: String,
    pub locked: bool,
    pub title: String,
    pub user: User,
    pub body: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub merged_at: Option<String>,
    pub merge_commit_sha: Option<String>,
    pub assignee: Option<User>,
    pub assignees: Vec<User>,
    pub requested_reviewers: Vec<Reviewer>,
    pub requested_teams: Vec<Team>,
    pub labels: Vec<Label>,
    pub milestone: Option<IgnoredAny>,
    /// `pull_request_review_comment` の payload には無い
    pub draft: Option<bool>,
    pub commits_url: Url,
    pub review_comments_url: Url,
    pub review_comment_url: Url,
    pub comments_url: Url,
    pub statuses_url: Url,
    pub head: PullRequestHead,
    pub base: PullRequestBase,
    #[serde(rename = "_links")]
    pub links: PullRequestLinks,
    pub author_association: String,
    pub active_lock_reason: Option<IgnoredAny>,
}

/// `pull_request_review` の `review`。
#[derive(Debug, Deserialize)]
pub struct Review {
    pub id: usize,
    pub node_id: String,
    pub user: User,
    /// approve にメッセージを付けなかった場合は null になる (#285)
    pub body: Option<String>,
    pub commit_id: String,
    pub submitted_at: Option<String>,
    /// `approved` / `changes_requested` / `commented` / `dismissed`。
    /// GitHub 側が値を増やしても deserialize に失敗しないよう、
    /// [`PullRequest::state`] と同じく String のまま持つ。
    pub state: String,
    pub html_url: Url,
    pub pull_request_url: Url,
    pub author_association: String,
    #[serde(rename = "_links")]
    pub links: Option<IgnoredAny>,
}

/// `pull_request_review_comment` の `comment`。diff 上の 1 コメント。
#[derive(Debug, Deserialize)]
pub struct ReviewComment {
    pub url: Url,
    pub pull_request_review_id: Option<usize>,
    pub id: usize,
    pub node_id: String,
    pub diff_hunk: String,
    pub path: String,
    pub commit_id: String,
    pub original_commit_id: String,
    pub user: User,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    pub html_url: Url,
    pub pull_request_url: Url,
    pub author_association: String,
    #[serde(rename = "_links")]
    pub links: Option<IgnoredAny>,
    pub reactions: Option<Reactions>,
    /// レビューコメントへの返信のときだけ入る。
    /// 通常のレビューコメントでは **キー自体が存在しない** (#122)。
    pub in_reply_to_id: Option<usize>,
    pub position: Option<usize>,
    pub original_position: Option<usize>,
    pub line: Option<usize>,
    pub original_line: Option<usize>,
    pub start_line: Option<usize>,
    pub original_start_line: Option<usize>,
    pub side: Option<String>,
    pub start_side: Option<String>,
    pub subject_type: Option<String>,
}

/// GitHub team。`requested_team` / `requested_teams` に入る。
///
/// team の review request を扱った payload-example が octokit に無く、実物で
/// 検証できていないため、`slug` などマッチに使うものだけ必須にして残りは Option。
/// 必須フィールドを増やすと deserialize 失敗で通知が止まる。
#[derive(Debug, Deserialize)]
pub struct Team {
    pub name: String,
    pub slug: String,
    pub id: usize,
    pub node_id: String,
    pub description: Option<String>,
    pub privacy: Option<String>,
    pub notification_setting: Option<String>,
    pub permission: Option<String>,
    pub url: Option<Url>,
    pub html_url: Option<Url>,
    pub members_url: Option<String>,
    pub repositories_url: Option<Url>,
    pub parent: Option<IgnoredAny>,
}

/// `pull_request.requested_reviewers` の 1 要素。
///
/// GitHub のスキーマ上ここは user と team が混ざりうる。untagged enum にすると
/// 1 フィールドの不一致で全体が失敗する (#311) ので、user なら `login`、
/// team なら `slug` が入る形で両方を受け取れる型にしている。
#[derive(Debug, Deserialize)]
pub struct Reviewer {
    /// user のときだけ入る
    pub login: Option<String>,
    /// team のときだけ入る
    pub slug: Option<String>,
    pub id: usize,
    pub node_id: String,
}

impl Reviewer {
    /// マッチに使う名前。user なら login、team なら slug。
    pub fn name(&self) -> Option<&str> {
        self.login.as_deref().or(self.slug.as_deref())
    }
}

#[derive(Debug, Deserialize)]
pub struct Label {
    pub id: usize,
    pub node_id: String,
    pub url: Url,
    pub name: String,
    pub color: String,
    pub default: bool,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Reactions {
    pub url: Url,
    pub total_count: usize,
    #[serde(rename(deserialize = "+1"))]
    pub plus_one: usize,
    #[serde(rename(deserialize = "-1"))]
    pub minus_one: usize,
    pub laugh: usize,
    pub hooray: usize,
    pub confused: usize,
    pub heart: usize,
    pub rocket: usize,
    pub eyes: usize,
}

#[derive(Debug, Deserialize)]
pub struct InstallationLite {
    pub id: usize,
    pub node_id: String,
}

impl Issue {
    pub fn is_pull_request(&self) -> bool {
        self.node_id.starts_with("PR_")
    }
}

impl<'a> From<&'a Label> for &'a str {
    fn from(label: &'a Label) -> &'a str {
        &label.name
    }
}

// Label と同じく、Rule::match_query_vec の `T: ToString` 境界に &User を
// 渡せるようにする。assignee のマッチ (#41) で使う。
impl std::fmt::Display for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.login)
    }
}

// Rule::match_query_vec の `T: ToString` 境界に &Label を渡すために必要。
// Display を実装しておけば std のブランケット実装経由で
// &Label: Display -> &Label: ToString が成り立つ。
impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}
