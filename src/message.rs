use crate::github;
use crate::slack;

use tracing::info;

impl TryFrom<&github::Payload> for slack::Message {
    type Error = ();

    fn try_from(payload: &github::Payload) -> Result<Self, Self::Error> {
        use github::Payload;

        match payload {
            Payload::Issues(issues) => {
                let i: &github::Issues = issues;
                i.try_into()
            }
            Payload::PullRequest(pr) => {
                let p: &github::PullRequest = pr;
                p.try_into()
            }
            Payload::IssueComment(ic) => {
                let ic: &github::IssueComment = ic;
                ic.try_into()
            }
            Payload::PullRequestReview(review) => {
                let r: &github::PullRequestReview = review;
                r.try_into()
            }
            Payload::PullRequestReviewComment(comment) => {
                let c: &github::PullRequestReviewComment = comment;
                c.try_into()
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

impl TryFrom<&github::Issues> for slack::Message {
    type Error = ();

    fn try_from(issues: &github::Issues) -> Result<Self, Self::Error> {
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
                        text += "\n*Assignees*\n";
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

impl TryFrom<&github::PullRequest> for slack::Message {
    type Error = ();

    fn try_from(pull_request: &github::PullRequest) -> Result<Self, Self::Error> {
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
                    let body = pr.body.as_deref().unwrap_or("");
                    let fallback = format!("{title}\n{body}", title = pr.title);

                    let mut text = body.to_string();
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

impl TryFrom<&github::IssueComment> for slack::Message {
    type Error = ();

    fn try_from(issue_comment: &github::IssueComment) -> Result<Self, Self::Error> {
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

impl TryFrom<&github::PullRequestReview> for slack::Message {
    type Error = ();

    fn try_from(review: &github::PullRequestReview) -> Result<Self, Self::Error> {
        // edited / dismissed は通知しない
        if review.action != github::PullRequestReviewAction::Submitted {
            return Err(());
        }

        let repo = &review.repository;
        let pr = &review.pull_request;
        let r = &review.review;
        let body = r.body.as_deref().unwrap_or("");

        let (verb, color) = match r.state.as_str() {
            "approved" => ("approved", slack::Color::Merged),
            "changes_requested" => ("requested changes on", slack::Color::Danger),
            "commented" => {
                // インラインコメントだけを submit すると、body が空の `commented`
                // review が飛んでくる。それ自体には情報が無く、個々のコメントは
                // pull_request_review_comment 側で通知されるので捨てる (#122)
                if body.is_empty() {
                    return Err(());
                }
                ("commented on", slack::Color::Comment)
            }
            // GitHub が state を増やしても落ちないように、未知の state は通知しない
            _ => return Err(()),
        };

        let text = format!(
            "[{repo}] {user} {verb} pull request <{link}|#{number}: {title}>",
            repo = repo.full_name,
            user = r.user.login,
            link = r.html_url,
            number = pr.number,
            title = pr.title,
        );

        let attach = slack::Attachment {
            title: None,
            title_link: None,
            // approve にメッセージを付けない運用もあるので、その場合は PR タイトルを出す
            fallback: if body.is_empty() {
                pr.title.clone()
            } else {
                body.to_string()
            },
            text: body.to_string(),
            color: Some(color),
        };

        Ok(Self {
            text,
            attachments: Some(vec![attach]),
        })
    }
}

impl TryFrom<&github::PullRequestReviewComment> for slack::Message {
    type Error = ();

    fn try_from(review_comment: &github::PullRequestReviewComment) -> Result<Self, Self::Error> {
        if review_comment.action != github::PullRequestReviewCommentAction::Created {
            return Err(());
        }

        let repo = &review_comment.repository;
        let pr = &review_comment.pull_request;
        let comment = &review_comment.comment;

        // 返信のときだけ in_reply_to_id が入る (#122)
        let kind = if comment.in_reply_to_id.is_some() {
            "New reply"
        } else {
            "New review comment"
        };

        let text = format!(
            "[{repo}] {kind} by {user} on pull request <{link}|#{number}: {title}>",
            repo = repo.full_name,
            user = comment.user.login,
            link = comment.html_url,
            number = pr.number,
            title = pr.title,
        );

        let attach = slack::Attachment {
            // どのファイルへのコメントかが分かるようにする
            title: Some(comment.path.clone()),
            title_link: Some(comment.html_url.clone()),
            fallback: comment.body.clone(),
            text: comment.body.clone(),
            color: Some(slack::Color::Comment),
        };

        Ok(Self {
            text,
            attachments: Some(vec![attach]),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::github::testing::de;
    use crate::slack;

    fn message(event: &str, test_json: &str) -> Result<slack::Message, ()> {
        let payload = de(event, test_json);
        (&payload).try_into()
    }

    /// #285: approve に付けたメッセージが Slack の本文に載ること。
    #[test]
    fn approved_review_notifies_with_body() {
        let msg = message(
            "pull_request_review",
            "pull_request_review.approved.derived.json",
        )
        .expect("approve は通知されるべき");

        assert!(msg.text.contains("approved"), "text = {}", msg.text);

        let attach = &msg.attachments.as_ref().unwrap()[0];
        assert!(attach.text.contains("@sksat"), "attach = {}", attach.text);
    }

    /// インラインコメントだけを submit したときに飛んでくる、
    /// body が空の `commented` review は通知しない (#122 側で個別に通知される)。
    #[test]
    fn commented_review_without_body_is_not_notified() {
        let msg = message("pull_request_review", "pull_request_review.submitted.json");
        assert!(msg.is_err(), "body が空の commented review は通知しない");
    }

    /// dismissed は通知しない。
    #[test]
    fn dismissed_review_is_not_notified() {
        let msg = message("pull_request_review", "pull_request_review.dismissed.json");
        assert!(msg.is_err(), "dismissed は通知しない");
    }

    /// #122: レビューコメントが本文付きで通知されること。
    #[test]
    fn review_comment_notifies_with_body() {
        let msg = message(
            "pull_request_review_comment",
            "pull_request_review_comment.created.with-organization.json",
        )
        .expect("レビューコメントは通知されるべき");

        let attach = &msg.attachments.as_ref().unwrap()[0];
        assert!(!attach.text.is_empty(), "本文が空");
        // どのファイルへのコメントかが分かること
        assert!(attach.title.is_some(), "path が入っていない");
    }
}
