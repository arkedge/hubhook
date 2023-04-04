use serde::Deserialize;
use url::Url;

#[derive(Debug, Deserialize)]
pub struct User<'a> {
    pub login: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct Repository<'a> {
    pub full_name: &'a str,
    #[serde(default)]
    pub topics: Vec<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct Issue<'a> {
    pub url: Url,
    pub html_url: Url,
    pub node_id: &'a str,
    pub number: usize,
    pub title: &'a str,
    pub user: User<'a>,
    pub labels: Vec<Label<'a>>,
    pub assignee: Option<User<'a>>,
    pub assignees: Vec<User<'a>>,
    pub body: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest<'a> {
    pub url: Url,
    pub html_url: Url,
    pub number: usize, // PR number
    pub title: &'a str,
    pub user: User<'a>,
    pub body: Option<&'a str>,
    pub assignees: Vec<User<'a>>,
    pub requested_reviewers: Vec<User<'a>>, // TODO: teamの場合もある
    pub requested_teams: Vec<()>,           // TODO: これは確実にteam
    pub labels: Vec<Label<'a>>,
}

#[derive(Debug, Deserialize)]
pub struct IssueComment<'a> {
    pub url: Url,
    pub html_url: Url,
    pub user: User<'a>,
    pub body: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct Label<'a> {
    pub name: &'a str,
}

impl Issue<'_> {
    pub fn is_pull_request(&self) -> bool {
        self.node_id.starts_with("PR_")
    }
}

impl<'a> From<&'a Label<'a>> for &'a str {
    fn from(label: &'a Label) -> &'a str {
        label.name
    }
}

impl ToString for &Label<'_> {
    fn to_string(&self) -> String {
        self.name.to_string()
    }
}
