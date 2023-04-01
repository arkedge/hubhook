use crate::github;
use crate::slack;

use tracing::info;

impl TryFrom<&github::Payload> for slack::Message {
    type Error = ();

    fn try_from(payload: &github::Payload) -> Result<Self, Self::Error> {
        use github::Payload;

        match payload {
            Payload::Issues(issues) => {
                let i: &github::IssuesEvent = issues;
                i.try_into()
            }
            Payload::PullRequest(pr) => {
                let p: &github::PullRequestEvent = pr;
                p.try_into()
            }
            Payload::IssueComment(ic) => {
                let ic: &github::IssueCommentEvent = ic;
                ic.try_into()
            }
        }
    }
}

fn users2str(assignees: &[github::common::User], delimiter: &str, to_link: bool) -> Option<String> {
    if assignees.is_empty() {
        return None;
    }

    Some(
        assignees
            .iter()
            .map(|a| a.login.to_string())
            .map(|a| {
                if to_link {
                    format!("<https://github.com/{a}|{a}>")
                } else {
                    a
                }
            })
            .collect::<Vec<String>>()
            .join(delimiter),
    )
}

impl TryFrom<&github::IssuesEvent> for slack::Message {
    type Error = ();

    fn try_from(issues: &github::IssuesEvent) -> Result<Self, Self::Error> {
        let repo = &issues.repository;
        let issue = &issues.issue;
        let user = &issue.user;

        match issues.action {
            github::IssuesAction::Opened => {
                // enterpriseでなければこっちに入る？
                // 2022-01-11: かと思ったがそうでもないようなのでログに出して様子を見る
                if let Some(assignee) = &issue.assignee {
                    let assign = &assignee.login;
                    info!("IssuesAction::Opened: issue.assignee = {assign}");
                }

                // ここにユーザへのリンクを入れるとGitHub Appが破壊するので入れない(#13)
                let text = format!(
                    "[{repo}] Issue created by {user}",
                    repo = repo.full_name,
                    user = user.login
                );

                let attach = {
                    let color = Some(slack::Color::Good);
                    let title = Some(format!(
                        "#{number} {title}",
                        number = issue.number,
                        title = issue.title
                    ));
                    let title_link = Some(issue.html_url.clone());

                    let mut text: String = issue.body.clone().unwrap_or_default();
                    if let Some(astr) = users2str(&issue.assignees, "\n", true) {
                        text += "\n*Asiggnees*\n";
                        text += &astr;
                    }

                    let fallback = format!(
                        "{title}\n{body}",
                        title = issue.title,
                        body = if let Some(b) = &issue.body { b } else { "" }
                    );

                    slack::Attachment {
                        title,
                        title_link,
                        fallback,
                        text,
                        color,
                    }
                };
                let attachments = Some(vec![attach]);

                Ok(Self { text, attachments })
            }
            github::IssuesAction::Assigned => {
                // enterpriseでなければこっちに入る？
                // 2022-01-11: かと思ったがそうでもないようなのでログに出して様子を見る
                if let Some(assignee) = &issue.assignee {
                    info!(
                        "IssuesAction::Assigned: issue.assignee = {}",
                        assignee.login
                    );
                }

                let assignees = &issue.assignees;
                assert!(!assignees.is_empty());

                let text = format!(
                    "[{}] Issue assigned to {}",
                    repo.full_name,
                    users2str(assignees, ",", false).expect("no assignees on issue assigned event")
                );

                let attach = {
                    let color = Some(slack::Color::Good);
                    let title = Some(format!(
                        "#{number} {title}",
                        number = issue.number,
                        title = issue.title
                    ));
                    let title_link = Some(issue.html_url.clone());
                    let fallback = issue.title.to_string();

                    let text = "*Assignees*\n".to_string()
                        + &users2str(assignees, "\n", true)
                            .expect("no assignees on issue assigned event");

                    slack::Attachment {
                        title,
                        title_link,
                        fallback,
                        text,
                        color,
                    }
                };
                let attachments = Some(vec![attach]);

                Ok(Self { text, attachments })
            }
            _ => Err(()),
        }
    }
}

impl TryFrom<&github::PullRequestEvent> for slack::Message {
    type Error = ();

    fn try_from(pull_request: &github::PullRequestEvent) -> Result<Self, Self::Error> {
        let repo = &pull_request.repository;
        let pr = &pull_request.pull_request;

        match pull_request.action {
            github::PullRequestAction::Opened => {
                let text = format!(
                    "[{repo}] Pull Request opened by {user}",
                    repo = repo.full_name,
                    user = pr.user.login
                );

                let attach = {
                    let color = Some(slack::Color::Good);

                    let title = Some(format!(
                        "#{number} {title}",
                        number = pr.number,
                        title = pr.title
                    ));
                    let title_link = Some(pr.html_url.clone());
                    let fallback = format!("{title}\n{body}", title = pr.title, body = pr.body);

                    let mut text = pr.body.clone();
                    if let Some(astr) = users2str(&pr.assignees, "\n", true) {
                        text += "\n*Asiggnees*\n";
                        text += &astr;
                    }

                    slack::Attachment {
                        title,
                        title_link,
                        fallback,
                        text,
                        color,
                    }
                };
                let attachments = Some(vec![attach]);

                Ok(Self { text, attachments })
            }

            github::PullRequestAction::Assigned => {
                let assignees = &pr.assignees;
                assert!(!assignees.is_empty());

                let text = {
                    let repo = &repo.full_name;
                    let assignees = users2str(assignees, ",", false)
                        .expect("no assignees on issue assigned event");
                    format!("[{repo}] Pull Request assigned to {assignees}",)
                };

                let attach = {
                    let title = Some(format!(
                        "#{number} {title}",
                        number = pr.number,
                        title = pr.title
                    ));
                    let title_link = Some(pr.html_url.clone());
                    let text = "*Assignees*\n".to_string()
                        + &users2str(assignees, "\n", true)
                            .expect("no assignees on puull request assigned event");

                    let color = Some(slack::Color::Good);

                    slack::Attachment {
                        title,
                        title_link,
                        fallback: pr.title.to_string(),
                        text,
                        color,
                    }
                };
                let attachments = Some(vec![attach]);

                Ok(Self { text, attachments })
            }
            _ => Err(()),
        }
    }
}

impl TryFrom<&github::IssueCommentEvent> for slack::Message {
    type Error = ();

    fn try_from(issue_comment: &github::IssueCommentEvent) -> Result<Self, Self::Error> {
        let repo = &issue_comment.repository;
        let issue = &issue_comment.issue;
        let comment = &issue_comment.comment;
        let ic_link = &comment.html_url;
        let username = &comment.user.login;

        match issue_comment.action {
            github::IssueCommentAction::Created => {
                let color = Some(slack::Color::Comment);

                let typ = if issue.is_pull_request() {
                    "pull request"
                } else {
                    "issue"
                };
                let text = format!(
                    "[{repo_name}] New comment by {username} on {typ} <{ic_link}|#{number}: {title}>",
                    repo_name = repo.full_name,
                    number = issue.number,
                    title = issue.title
                );
                let attach = slack::Attachment {
                    title: None,
                    title_link: None,
                    fallback: comment.body.clone(),
                    text: comment.body.clone(),
                    color,
                };
                let attachments = Some(vec![attach]);

                Ok(Self { text, attachments })
            }
            _ => Err(()),
        }
    }
}
