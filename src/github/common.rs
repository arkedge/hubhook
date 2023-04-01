use serde::Deserialize;
use url::Url;

#[derive(Debug, Deserialize)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct Repository {
    pub full_name: String,
    #[serde(default)]
    pub topics: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Issue {
    pub url: Url,
    pub html_url: Url,
    pub node_id: String,
    pub number: usize,
    pub title: String,
    pub user: User,
    pub labels: Vec<Label>,
    pub assignee: Option<User>,
    pub assignees: Vec<User>,
    pub body: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub url: Url,
    pub html_url: Url,
    pub number: usize, // PR number
    pub title: String,
    pub user: User,
    pub body: String,
    pub assignees: Vec<User>,
    pub requested_reviewers: Vec<User>, // TODO: teamの場合もある
    pub requested_teams: Vec<()>,       // TODO: これは確実にteam
    pub labels: Vec<Label>,
}

#[derive(Debug, Deserialize)]
pub struct IssueComment {
    pub url: Url,
    pub html_url: Url,
    pub user: User,
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct Label {
    pub name: String,
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
