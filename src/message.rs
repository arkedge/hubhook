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

/// 本文と Assignees から attachment の blocks を作る。
///
/// 1 つのブロックにまとめるが、Assignees は「必ず残したい末尾」として渡す。
/// 連結してから切ると、本文が長いときに Assignees が消える。
/// 本文も Assignees も無ければブロックを作らない (空の text は拒否される)。
/// リンクの記法。同じ内容でも、入れる場所によって解釈される方言が違う。
///
/// - markdown ブロック: `[text](url)`
/// - attachment の `text` (mrkdwn): `<url|text>`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkStyle {
    Mrkdwn,
    Markdown,
}

impl LinkStyle {
    fn link(self, text: &str, url: &str) -> String {
        match self {
            Self::Mrkdwn => format!("<{url}|{text}>"),
            Self::Markdown => format!("[{text}]({url})"),
        }
    }
}

/// Assignees の行。入れる場所の方言に合わせて作る。
fn assignees_suffix(assignees: &[github::common::User], style: LinkStyle, bold: &str) -> String {
    users2str(assignees, "\n", Some(style))
        .map(|a| format!("\n{bold}Assignees{bold}\n{a}"))
        .unwrap_or_default()
}

/// attachment の本文 (markdown ブロック)。
fn body_blocks(body: &str, assignees: &[github::common::User]) -> Vec<slack::Block> {
    let suffix = assignees_suffix(assignees, LinkStyle::Markdown, "**");

    slack::Block::markdown(body, &suffix).into_iter().collect()
}

/// ブロックが拒否されたときの退避先 (mrkdwn)。
///
/// ブロックの Markdown をそのまま `text` に入れると、`**太字**` や
/// `[name](url)` が解釈されず、従来より悪い表示になる。方言が違うので
/// 使い回せない。本文は元から生のままだったので、Assignees だけ作り直す。
fn body_text(body: &str, assignees: &[github::common::User]) -> Option<String> {
    let suffix = assignees_suffix(assignees, LinkStyle::Mrkdwn, "*");

    if body.trim().is_empty() && suffix.trim().is_empty() {
        return None;
    }

    Some(format!("{body}{suffix}"))
}

/// attachment の本文。markdown ブロックと、退避用の mrkdwn を組にする。
fn body_content(body: &str, assignees: &[github::common::User]) -> slack::Body {
    slack::Body::new(body_blocks(body, assignees), body_text(body, assignees))
}

fn users2str(
    assignees: &[github::common::User],
    delimiter: &str,
    style: Option<LinkStyle>,
) -> Option<String> {
    if assignees.is_empty() {
        return None;
    }

    Some(
        assignees
            .iter()
            .map(|a| match style {
                Some(style) => style.link(&a.login, a.html_url.as_str()),
                None => a.login.clone(),
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

                    let fallback = format!(
                        "{title}\n{body}",
                        title = issue.title,
                        body = if let Some(b) = &issue.body { b } else { "" }
                    );

                    slack::Attachment {
                        title,
                        title_link,
                        fallback,
                        body: body_content(issue.body.as_deref().unwrap_or(""), &issue.assignees),
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
                    users2str(assignees, ", ", None).expect("no assignees on issue assigned event")
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

                    slack::Attachment {
                        title,
                        title_link,
                        fallback,
                        body: body_content("", assignees),
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

                    slack::Attachment {
                        title,
                        title_link,
                        fallback,
                        body: body_content(body, &pr.assignees),
                        color,
                    }
                };
                let attachments = Some(vec![attach]);

                Ok(Self { text, attachments })
            }

            // #87: review を依頼されたことを通知する
            github::PullRequestAction::ReviewRequested => {
                let requested = match (
                    &pull_request.requested_reviewer,
                    &pull_request.requested_team,
                ) {
                    (Some(user), _) => user.login.clone(),
                    (None, Some(team)) => format!("team {slug}", slug = team.slug),
                    // user も team も無い payload は想定していないので通知しない
                    (None, None) => return Err(()),
                };

                let text = format!(
                    "[{repo}] {sender} requested a review from {requested}",
                    repo = repo.full_name,
                    sender = pull_request.sender.login,
                );

                let attach = {
                    let title = Some(format!(
                        "#{number} {title}",
                        number = pr.number,
                        title = pr.title
                    ));
                    let title_link = Some(pr.html_url.clone());

                    slack::Attachment {
                        title,
                        title_link,
                        fallback: pr.title.to_string(),
                        body: body_content(pr.body.as_deref().unwrap_or(""), &[]),
                        // 「対応してほしい」通知なので opened / assigned とは色を変える
                        color: Some(slack::Color::Warning),
                    }
                };

                Ok(Self {
                    text,
                    attachments: Some(vec![attach]),
                })
            }

            github::PullRequestAction::Assigned => {
                let assignees = &pr.assignees;
                assert!(!assignees.is_empty());

                let text = {
                    let repo = &repo.full_name;
                    let assignees = users2str(assignees, ", ", None)
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
                    let color = Some(slack::Color::Good);

                    slack::Attachment {
                        title,
                        title_link,
                        fallback: pr.title.to_string(),
                        body: body_content("", assignees),
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
                    body: body_content(&comment.body, &[]),
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

        // approve にメッセージを付けない運用もある。その場合 text が空だと
        // Slack の添付がほぼ空表示になるので、PR タイトルを出す
        let attach_text = if body.is_empty() {
            pr.title.clone()
        } else {
            body.to_string()
        };

        let attach = slack::Attachment {
            title: None,
            title_link: None,
            fallback: attach_text.clone(),
            body: body_content(&attach_text, &[]),
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
            body: body_content(&comment.body, &[]),
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
        assert!(
            attach.body.blocks()[0].text().contains("@sksat"),
            "attach = {}",
            attach.body.blocks()[0].text()
        );
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

    /// #122: レビューコメントへの返信が "New reply" として通知され、
    /// 本文が保たれること。
    #[test]
    fn review_comment_reply_notifies_as_reply() {
        let msg = message(
            "pull_request_review_comment",
            "pull_request_review_comment.reply.derived.json",
        )
        .expect("返信も通知されるべき");

        assert!(msg.text.contains("New reply"), "text = {}", msg.text);

        let attach = &msg.attachments.as_ref().unwrap()[0];
        assert!(
            attach.body.blocks()[0].text().contains("@sksat"),
            "attach = {}",
            attach.body.blocks()[0].text()
        );
    }

    /// 通常のレビューコメントは "New review comment" になること。
    #[test]
    fn plain_review_comment_is_not_labeled_as_reply() {
        let msg = message(
            "pull_request_review_comment",
            "pull_request_review_comment.created.with-organization.json",
        )
        .expect("レビューコメントは通知されるべき");

        assert!(
            msg.text.contains("New review comment"),
            "text = {}",
            msg.text
        );
        assert!(!msg.text.contains("New reply"), "text = {}", msg.text);
    }

    /// #285: approve にメッセージを付けない場合、attachment の本文が
    /// 空にならず PR タイトルが入ること。
    #[test]
    fn approved_review_without_body_falls_back_to_title() {
        let msg = message(
            "pull_request_review",
            "pull_request_review.approved.no-body.derived.json",
        )
        .expect("メッセージ無しの approve も通知されるべき");

        let attach = &msg.attachments.as_ref().unwrap()[0];
        assert!(
            !attach.body.blocks()[0].text().is_empty(),
            "attachment の本文が空"
        );
        assert_eq!(attach.body.blocks()[0].text(), attach.fallback);
    }

    /// 本文を markdown ブロックとして、変換せずそのまま渡すこと。
    ///
    /// attachment の `text` (mrkdwn) に入れていた頃は `##` がそのまま出て、
    /// `*x*` の強調も入れ替わっていた。markdown ブロックは本物の Markdown を
    /// 解釈するので、GitHub の本文を加工せずに渡す。
    #[test]
    fn body_is_passed_through_as_markdown() {
        let msg = message(
            "pull_request_review",
            "pull_request_review.approved.derived.json",
        )
        .expect("通知されるべき");

        let attach = &msg.attachments.as_ref().unwrap()[0];
        assert_eq!(attach.body.blocks().len(), 1, "markdown ブロックが 1 つ");

        // fixture の review 本文がそのまま入っていること
        let crate::github::Payload::PullRequestReview(review) = &de(
            "pull_request_review",
            "pull_request_review.approved.derived.json",
        ) else {
            panic!("not a review");
        };
        let body = review.review.body.as_deref().unwrap();
        assert_eq!(attach.body.blocks()[0].text(), body, "加工されている");
    }

    /// attachment 本文の Assignees が Markdown 記法になること。
    ///
    /// markdown ブロックの中では `*x*` は斜体、リンクは `[text](url)` なので、
    /// mrkdwn のまま (`*Assignees*` / `<url|x>`) だと崩れる。
    #[test]
    fn assignees_in_body_use_markdown_syntax() {
        let msg = message(
            "pull_request",
            "pull_request.assigned.with-organization.json",
        )
        .expect("通知されるべき");

        let body = msg.attachments.as_ref().unwrap()[0].body.blocks()[0].text();
        assert!(
            body.contains("**Assignees**"),
            "太字が Markdown でない: {body}"
        );
        assert!(
            body.contains("[Codertocat](https://github.com/Codertocat)"),
            "リンクが Markdown でない: {body}"
        );
    }

    /// 主となるブロックは Markdown、退避先は mrkdwn になること。
    ///
    /// 退避時にブロックの Markdown をそのまま `text` に入れると、
    /// `**太字**` や `[name](url)` が解釈されず、従来より悪い表示になる。
    #[test]
    fn block_is_markdown_and_fallback_is_mrkdwn() {
        let msg = message(
            "pull_request",
            "pull_request.assigned.with-organization.json",
        )
        .expect("通知されるべき");
        let a = &msg.attachments.as_ref().unwrap()[0];

        // 主: markdown ブロック
        let block = a.body.blocks()[0].text();
        assert!(block.contains("**Assignees**"), "block = {block}");
        assert!(
            block.contains("[Codertocat](https://github.com/Codertocat)"),
            "block = {block}"
        );

        // 退避: mrkdwn
        let text = a.body.mrkdwn().expect("退避先が無い");
        assert!(
            !text.contains("**Assignees**"),
            "Markdown のままになっている: {text}"
        );
        assert!(text.contains("*Assignees*"), "text = {text}");
        assert!(
            text.contains("<https://github.com/Codertocat|Codertocat>"),
            "mrkdwn のリンクになっていない: {text}"
        );
    }

    /// 本文が無い PR でもブロックを作らず、通知は飛ぶこと。
    ///
    /// 空の `text` を持つ markdown ブロックを送ると `invalid_blocks` で
    /// 拒否され、通知そのものが飛ばなくなる。本文なしの PR は珍しくない。
    #[test]
    fn bodyless_pull_request_makes_no_block() {
        let msg = message(
            "pull_request",
            "pull_request.review_requested.no-body.derived.json",
        )
        .expect("本文が無くても通知されるべき");

        let attach = &msg.attachments.as_ref().unwrap()[0];
        assert!(
            attach.body.blocks().is_empty(),
            "空のブロックを作っている: {:?}",
            attach.body.blocks()
        );
        // 本文が無くても要約行は出る
        assert!(
            msg.text.contains("requested a review"),
            "text = {}",
            msg.text
        );
    }

    /// 本文が長くても Assignees が消えないこと。
    #[test]
    fn assignees_survive_a_long_body() {
        let msg = message(
            "pull_request",
            "pull_request.assigned.with-organization.json",
        )
        .expect("通知されるべき");

        let body = msg.attachments.as_ref().unwrap()[0].body.blocks()[0].text();
        assert!(body.contains("**Assignees**"), "Assignees が無い: {body}");
    }

    /// #87: review request が通知されること。
    #[test]
    fn review_requested_notifies() {
        let msg = message("pull_request", "pull_request.review_requested.json")
            .expect("review request は通知されるべき");
        assert!(
            msg.text.contains("requested a review from octocat"),
            "text = {}",
            msg.text
        );
    }

    /// #87: team への review request が team 名で通知されること。
    #[test]
    fn team_review_request_notifies_with_slug() {
        let msg = message(
            "pull_request",
            "pull_request.review_requested.team.derived.json",
        )
        .expect("team への review request も通知されるべき");

        assert!(
            msg.text.contains("requested a review from team octo-team"),
            "text = {}",
            msg.text
        );
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
        assert!(!attach.body.blocks()[0].text().is_empty(), "本文が空");
        // どのファイルへのコメントかが分かること
        assert!(attach.title.is_some(), "path が入っていない");
    }
}
