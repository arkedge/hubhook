use crate::github;
use crate::slack;

use tracing::info;

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

pub enum CreatedMessage {
    Ok(slack::Message),
    SkipThisEvent,
}

impl CreatedMessage {
    pub fn as_ok(&self) -> Option<&slack::Message> {
        if let Self::Ok(v) = self {
            Some(v)
        } else {
            None
        }
    }
}

pub trait IntoMessage {
    fn create_message(&self) -> CreatedMessage;
}

impl IntoMessage for github::IssuesEvent<'_> {
    fn create_message(&self) -> CreatedMessage {
        let repo = &self.repository;
        let issue = &self.issue;
        let user = &issue.user;

        match self.action {
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

                    let mut text: String = issue.body.map(|s| s.to_string()).unwrap_or_default();
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

                CreatedMessage::Ok(slack::Message { text, attachments })
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

                CreatedMessage::Ok(slack::Message { text, attachments })
            }
            _ => CreatedMessage::SkipThisEvent,
        }
    }
}

impl IntoMessage for github::PullRequestEvent<'_> {
    fn create_message(&self) -> CreatedMessage {
        let repo = &self.repository;
        let pr = &self.pull_request;

        match self.action {
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
                    let fallback = format!(
                        "{title}\n{body}",
                        title = pr.title,
                        body = pr.body.unwrap_or_default()
                    );

                    let mut text = match pr.body {
                        Some(s) => s.to_string(),
                        None => String::default(),
                    };
                    if let Some(astr) = users2str(&pr.assignees, "\n", true) {
                        text += "\n*Assignees*\n";
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

                CreatedMessage::Ok(slack::Message { text, attachments })
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

                CreatedMessage::Ok(slack::Message { text, attachments })
            }
            _ => CreatedMessage::SkipThisEvent,
        }
    }
}

impl IntoMessage for github::IssueCommentEvent<'_> {
    fn create_message(&self) -> CreatedMessage {
        let repo = &self.repository;
        let issue = &self.issue;
        let comment = &self.comment;
        let ic_link = &comment.html_url;
        let username = &comment.user.login;

        match self.action {
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
                    fallback: comment.body.to_string(),
                    text: comment.body.to_string(),
                    color,
                };
                let attachments = Some(vec![attach]);

                CreatedMessage::Ok(slack::Message { text, attachments })
            }
            _ => CreatedMessage::SkipThisEvent,
        }
    }
}
