use serde::Deserialize;
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
    pub description: String,
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
    pub created_at: String, // 2021-10-27T05:00:55Z
    pub updated_at: String,
    pub pushed_at: String,
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
    pub disabled: bool,
    pub open_issues_count: usize,
    pub license: Option<License>,
    pub allow_forking: bool,
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
    pub labels: Vec<Label>,
    pub state: String,
    pub locked: bool,
    pub assignee: Option<User>,
    pub assignees: Vec<User>,
    pub minestone: Option<()>,
    pub comments: usize,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub author_association: String,
    pub active_lock_reason: Option<()>,
    pub body: String,
    pub reactions: Reactions,
    pub timeline_url: Url,
    pub performed_via_github_app: Option<()>,
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
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub merged_at: Option<String>,
    pub merge_commit_sha: String,
    pub assignee: Option<User>, // Issueと挙動が違う？
    pub assignees: Vec<User>,
    pub requested_reviewers: Vec<User>, // TODO: teamの場合もある
    pub requested_teams: Vec<()>,       // TODO: これは確実にteam
    pub labels: Vec<Label>,
    pub minestone: Option<()>,
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
    pub auto_merge: Option<()>,
    pub active_lock_reason: Option<()>,
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
    pub repo: Repository,
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
    pub performed_via_github_app: Option<()>,
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

impl ToString for &Label {
    fn to_string(&self) -> String {
        self.name.to_string()
    }
}
