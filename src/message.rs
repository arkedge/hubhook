use crate::github;
use crate::slack;

use tracing::info;

/// `slack::Message` を作らなかった理由。
///
/// 「通知しないと決めているケース」と「想定が崩れているケース」を区別する。
/// 区別せずに全部 `error!` で出すと、本物の失敗が埋もれる。issues は 16 個の
/// action のうち 2 個しか通知対象にしていないので、大半が前者になる。
#[derive(Debug)]
pub enum NotRendered {
    /// 通知対象にしていない action / state。想定どおりの動作。
    Skipped(String),
    /// レンダリングに必要な情報が payload に無い。想定が崩れている。
    Unexpected(String),
}

impl std::fmt::Display for NotRendered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skipped(why) | Self::Unexpected(why) => write!(f, "{why}"),
        }
    }
}

impl TryFrom<&github::Payload> for slack::Message {
    type Error = NotRendered;

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

/// リンクの記法。
///
/// トップレベルの `text` も attachment の本文も mrkdwn なので `<url|text>`。
/// 本文の GitHub markdown は [`crate::mrkdwn`] で変換してから入れるため、
/// `[text](url)` を組み立てる場所は無い。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkStyle {
    Mrkdwn,
}

impl LinkStyle {
    fn link(self, text: &str, url: &str) -> String {
        match self {
            Self::Mrkdwn => format!("<{url}|{text}>"),
        }
    }
}

/// リポジトリ名をリンクにする (トップレベルの `text` 用)。
fn repo_link(repo: &github::common::Repository) -> String {
    LinkStyle::Mrkdwn.link(&repo.full_name, repo.html_url.as_str())
}

/// アカウント名をリンクにする (トップレベルの `text` 用)。
fn user_link(user: &github::common::User) -> String {
    LinkStyle::Mrkdwn.link(&user.login, user.html_url.as_str())
}

/// Assignees の行。入れる場所の方言に合わせて作る。
///
/// 1 人 1 行で並べると人数の分だけ縦に伸びて本文が見えなくなるので、
/// `Assignees: a, b, c` の 1 行にまとめる。
fn assignees_line(
    assignees: &[github::common::User],
    style: LinkStyle,
    bold: &str,
) -> Option<String> {
    users2str(assignees, ", ", Some(style)).map(|a| format!("{bold}Assignees{bold}: {a}"))
}

/// 本文の後ろに Assignees を足す。
///
/// 空行で区切る。1 行だけだと本文の続きに見えて、どこまでが本文か分からない。
/// 本文が無いときは区切りを入れない (先頭が空行になり、その分だけ縦に伸びる)。
fn with_assignees(body: &str, line: Option<String>) -> String {
    let Some(line) = line else {
        return body.to_string();
    };

    if body.trim().is_empty() {
        return line;
    }

    format!("{body}\n\n{line}")
}

/// attachment の本文。
///
/// GitHub の本文を mrkdwn に変換して入れる。attachment の中では `blocks` が
/// 一切通らない (`markdown` は `internal_error`、`rich_text` と `section` は
/// `invalid_attachments`) ので、色バーを残すにはこの形しかない。
///
/// 長さでは切らない。Slack は 40,000 文字を超えたメッセージを切る
/// (`chat.postMessage` のリファレンス) ので、それを超える本文では末尾の
/// Assignees が消えることがある。escape で `&` が `&amp;` になる分だけ
/// 変換後は伸びるが、普通の文では 1% も増えないので 39,000 文字ほどの本文が
/// 必要になる。
///
/// こちらで切らないのは、切る位置が `<url|label>` やコードフェンスの途中に
/// なると記法が壊れるため。閉じないフェンスは以降の本文と Assignees を
/// 飲み込むので、切られるより悪い。
fn body_text(body: &str, assignees: &[github::common::User]) -> Option<String> {
    let body = crate::mrkdwn::from_markdown(body);
    let text = with_assignees(&body, assignees_line(assignees, LinkStyle::Mrkdwn, "*"));

    (!text.trim().is_empty()).then_some(text)
}

/// 誰の操作かを footer に出す (#289)。
///
/// アイコンも添えると一目で分かる。footer は mrkdwn が効かないので、
/// login をそのまま置く (リンクにはできない)。
fn sender_footer(sender: &github::common::User) -> slack::Footer {
    slack::Footer {
        footer: sender.login.clone(),
        footer_icon: Some(sender.avatar_url.clone()),
    }
}

fn body_content(body: &str, assignees: &[github::common::User]) -> slack::Body {
    slack::Body::new(body_text(body, assignees))
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
    type Error = NotRendered;

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

                // 以前はここにリンクを入れると unfurl でメッセージが崩れていたため
                // 避けていたが、post 時に unfurl を明示的に切ったのでリンクにできる
                let text = format!(
                    "[{repo}] Issue created by {user}",
                    repo = repo_link(repo),
                    user = user_link(user)
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
                        footer: Some(sender_footer(&issues.sender)),
                        body: body_content(issue.body.as_deref().unwrap_or(""), &issue.assignees),
                        color,
                        ..Default::default()
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
                    repo_link(repo),
                    users2str(assignees, ", ", Some(LinkStyle::Mrkdwn))
                        .expect("no assignees on issue assigned event")
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
                        footer: Some(sender_footer(&issues.sender)),
                        body: body_content("", assignees),
                        color,
                        ..Default::default()
                    }
                };
                let attachments = Some(vec![attach]);

                Ok(Self { text, attachments })
            }
            _ => Err(NotRendered::Skipped(format!(
                "issues action {:?}",
                issues.action
            ))),
        }
    }
}

impl TryFrom<&github::PullRequest> for slack::Message {
    type Error = NotRendered;

    fn try_from(pull_request: &github::PullRequest) -> Result<Self, Self::Error> {
        let repo = &pull_request.repository;
        let pr = &pull_request.pull_request;

        match pull_request.action {
            github::PullRequestAction::Opened => {
                let text = format!(
                    "[{repo}] Pull Request opened by {user}",
                    repo = repo_link(repo),
                    user = user_link(&pr.user)
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
                        footer: Some(sender_footer(&pull_request.sender)),
                        body: body_content(body, &pr.assignees),
                        color,
                        ..Default::default()
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
                    (Some(user), _) => user_link(user),
                    (None, Some(team)) => match &team.html_url {
                        Some(url) => LinkStyle::Mrkdwn
                            .link(&format!("team {slug}", slug = team.slug), url.as_str()),
                        None => format!("team {slug}", slug = team.slug),
                    },
                    // user も team も無い payload は想定していないので通知しない
                    (None, None) => {
                        return Err(NotRendered::Unexpected(
                            "review request has neither requested_reviewer nor requested_team"
                                .to_string(),
                        ));
                    }
                };

                let text = format!(
                    "[{repo}] {sender} requested a review from {requested}",
                    repo = repo_link(repo),
                    sender = user_link(&pull_request.sender),
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
                        footer: Some(sender_footer(&pull_request.sender)),
                        body: body_content(pr.body.as_deref().unwrap_or(""), &[]),
                        // 「対応してほしい」通知なので opened / assigned とは色を変える
                        color: Some(slack::Color::Warning),
                        ..Default::default()
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
                    let repo = repo_link(repo);
                    let assignees = users2str(assignees, ", ", Some(LinkStyle::Mrkdwn))
                        .expect("no assignees on pull request assigned event");
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
                        footer: Some(sender_footer(&pull_request.sender)),
                        body: body_content("", assignees),
                        color,
                        ..Default::default()
                    }
                };
                let attachments = Some(vec![attach]);

                Ok(Self { text, attachments })
            }
            _ => Err(NotRendered::Skipped(format!(
                "pull_request action {:?}",
                pull_request.action
            ))),
        }
    }
}

impl TryFrom<&github::IssueComment> for slack::Message {
    type Error = NotRendered;

    fn try_from(issue_comment: &github::IssueComment) -> Result<Self, Self::Error> {
        let repo = &issue_comment.repository;
        let issue = &issue_comment.issue;
        let comment = &issue_comment.comment;
        let ic_link = &comment.html_url;

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
                    repo_name = repo_link(repo),
                    username = user_link(&comment.user),
                    number = issue.number,
                    title = issue.title
                );
                let attach = slack::Attachment {
                    title: None,
                    title_link: None,
                    fallback: comment.body.clone(),
                    footer: Some(sender_footer(&issue_comment.sender)),
                    body: body_content(&comment.body, &[]),
                    color,
                    ..Default::default()
                };
                let attachments = Some(vec![attach]);

                Ok(Self { text, attachments })
            }
            _ => Err(NotRendered::Skipped(format!(
                "issue_comment action {:?}",
                issue_comment.action
            ))),
        }
    }
}

impl TryFrom<&github::PullRequestReview> for slack::Message {
    type Error = NotRendered;

    fn try_from(review: &github::PullRequestReview) -> Result<Self, Self::Error> {
        // edited / dismissed は通知しない
        if review.action != github::PullRequestReviewAction::Submitted {
            return Err(NotRendered::Skipped(format!(
                "pull_request_review action {:?}",
                review.action
            )));
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
                    return Err(NotRendered::Skipped(
                        "pull_request_review with empty body".to_string(),
                    ));
                }
                ("commented on", slack::Color::Comment)
            }
            // GitHub が state を増やしても落ちないように、未知の state は通知しない
            _ => {
                return Err(NotRendered::Skipped(format!(
                    "unknown pull_request_review state {:?}",
                    r.state
                )));
            }
        };

        let text = format!(
            "[{repo}] {user} {verb} pull request <{link}|#{number}: {title}>",
            repo = repo_link(repo),
            user = user_link(&r.user),
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
            footer: Some(sender_footer(&review.sender)),
            body: body_content(&attach_text, &[]),
            color: Some(color),
            ..Default::default()
        };

        Ok(Self {
            text,
            attachments: Some(vec![attach]),
        })
    }
}

impl TryFrom<&github::PullRequestReviewComment> for slack::Message {
    type Error = NotRendered;

    fn try_from(review_comment: &github::PullRequestReviewComment) -> Result<Self, Self::Error> {
        if review_comment.action != github::PullRequestReviewCommentAction::Created {
            return Err(NotRendered::Skipped(format!(
                "pull_request_review_comment action {:?}",
                review_comment.action
            )));
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
            repo = repo_link(repo),
            user = user_link(&comment.user),
            link = comment.html_url,
            number = pr.number,
            title = pr.title,
        );

        let attach = slack::Attachment {
            // どのファイルへのコメントかが分かるようにする
            title: Some(comment.path.clone()),
            title_link: Some(comment.html_url.clone()),
            fallback: comment.body.clone(),
            footer: Some(sender_footer(&review_comment.sender)),
            body: body_content(&comment.body, &[]),
            color: Some(slack::Color::Comment),
            ..Default::default()
        };

        Ok(Self {
            text,
            attachments: Some(vec![attach]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{LinkStyle, NotRendered, assignees_line, with_assignees};
    use crate::github::testing::{de, de_without};
    use crate::slack;

    fn message(event: &str, test_json: &str) -> Result<slack::Message, NotRendered> {
        let payload = de(event, test_json);
        (&payload).try_into()
    }

    /// 一部のフィールドを落とした payload で試す。
    fn message_without(
        event: &str,
        test_json: &str,
        keys: &[&str],
    ) -> Result<slack::Message, NotRendered> {
        let payload = de_without(event, test_json, keys);
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
            attach.body.text().expect("本文が無い").contains("@sksat"),
            "attach = {}",
            attach.body.text().expect("本文が無い")
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
            attach.body.text().expect("本文が無い").contains("@sksat"),
            "attach = {}",
            attach.body.text().expect("本文が無い")
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
            !attach.body.text().expect("本文が無い").is_empty(),
            "attachment の本文が空"
        );
        assert_eq!(attach.body.text().expect("本文が無い"), attach.fallback);
    }

    /// 本文が mrkdwn に変換されて入ること。
    ///
    /// GitHub の本文をそのまま入れると `##` や `**` が生で出る。attachment の
    /// 中では blocks が通らないので、こちらで mrkdwn にしてから入れる。
    #[test]
    fn body_is_converted_to_mrkdwn() {
        let msg = message(
            "pull_request_review",
            "pull_request_review.approved.derived.json",
        )
        .expect("通知されるべき");

        let attach = &msg.attachments.as_ref().unwrap()[0];

        // 装飾の無い本文は変換しても変わらないこと
        let crate::github::Payload::PullRequestReview(review) = &de(
            "pull_request_review",
            "pull_request_review.approved.derived.json",
        ) else {
            panic!("not a review");
        };
        let body = review.review.body.as_deref().unwrap();
        assert_eq!(
            attach.body.text().expect("本文が無い"),
            body,
            "加工されている"
        );
    }

    /// attachment 本文の Assignees が mrkdwn 記法になること。
    ///
    /// attachment の本文は mrkdwn なので、`**x**` や `[text](url)` を入れると
    /// 生のまま出る。
    #[test]
    fn assignees_in_body_use_mrkdwn_syntax() {
        let msg = message(
            "pull_request",
            "pull_request.assigned.with-organization.json",
        )
        .expect("通知されるべき");

        let body = msg.attachments.as_ref().unwrap()[0]
            .body
            .text()
            .expect("本文が無い");
        assert!(body.contains("*Assignees*: "), "1 行になっていない: {body}");
        assert!(
            body.contains("<https://github.com/Codertocat|Codertocat>"),
            "リンクが mrkdwn でない: {body}"
        );
    }

    /// footer に sender と avatar が出ること (#289)。
    ///
    /// issue の作成者ではなく**操作した人**を出す。comment なら
    /// コメントした人で、issue の作者とは別人になる。
    #[test]
    fn footer_shows_the_sender() {
        let payload = de(
            "pull_request_review_comment",
            "pull_request_review_comment.created.with-organization.json",
        );
        let sender = payload.sender().login.clone();

        let msg = slack::Message::try_from(&payload).expect("通知されるべき");
        let footer = msg.attachments.as_ref().unwrap()[0]
            .footer
            .as_ref()
            .expect("footer が無い");

        assert_eq!(footer.footer, sender);
        assert!(footer.footer_icon.is_some(), "avatar が無い");
    }

    /// 本文が無いときに Assignees の前で空行を作らないこと。
    ///
    /// 空行の分だけ通知が縦に伸びる。assigned は本文を出さないので必ず通る。
    #[test]
    fn assignees_without_body_have_no_leading_blank_line() {
        let msg = message(
            "pull_request",
            "pull_request.assigned.with-organization.json",
        )
        .expect("通知されるべき");
        let a = &msg.attachments.as_ref().unwrap()[0];

        let text = a.body.text().expect("本文が無い");
        assert!(text.starts_with('*'), "空行から始まっている: {text:?}");
    }

    /// 本文と Assignees が空行で区切られること。
    ///
    /// 1 行だけだと本文の続きに見えて、どこまでが本文か分からない。
    #[test]
    fn assignees_are_separated_from_the_body_by_a_blank_line() {
        assert_eq!(
            with_assignees("本文", Some("*Assignees*: sksat".to_string())),
            "本文\n\n*Assignees*: sksat"
        );
        assert_eq!(
            with_assignees("本文", None),
            "本文",
            "余計な改行が付いている"
        );
    }

    /// Assignees が 1 行にまとまること。
    ///
    /// 1 人 1 行で並べると人数の分だけ縦に伸びて、本文が見えなくなる。
    #[test]
    fn assignees_are_on_one_line() {
        let payload = de(
            "pull_request",
            "pull_request.assigned.two-assignees.derived.json",
        );
        let assignees = payload.assignees();
        assert_eq!(assignees.len(), 2, "2 人以上の fixture が必要");

        let style = LinkStyle::Mrkdwn;
        let line = assignees_line(assignees, style, "*").expect("Assignees が無い");

        assert!(!line.contains('\n'), "1 行に収まっていない: {line:?}");
        for (login, url) in [
            ("Codertocat", "https://github.com/Codertocat"),
            ("octocat", "http://github.com/octocat"),
        ] {
            let link = style.link(login, url);
            assert!(line.contains(&link), "{link} が無い: {line}");
        }
    }

    /// repo とアカウントがトップレベルの text でリンクになること。
    ///
    /// トップレベルは mrkdwn なので `<url|text>` 記法。
    #[test]
    fn repo_and_user_are_linked_in_text() {
        let msg = message(
            "pull_request_review_comment",
            "pull_request_review_comment.created.with-organization.json",
        )
        .expect("通知されるべき");

        assert!(
            msg.text.contains("|Codertocat/Hello-World>"),
            "repo がリンクになっていない: {}",
            msg.text
        );
        assert!(
            msg.text.contains("|Codertocat>"),
            "アカウントがリンクになっていない: {}",
            msg.text
        );
    }

    /// 本文が無い PR では本文を入れず、通知は飛ぶこと。
    ///
    /// 空の `text` を送ると `no_text` で拒否され、通知そのものが飛ばなく
    /// なる。本文なしの PR は珍しくない。
    #[test]
    fn bodyless_pull_request_sends_no_text() {
        let msg = message(
            "pull_request",
            "pull_request.review_requested.no-body.derived.json",
        )
        .expect("本文が無くても通知されるべき");

        let attach = &msg.attachments.as_ref().unwrap()[0];
        assert!(
            attach.body.text().is_none(),
            "空の本文を入れている: {:?}",
            attach.body
        );
        // 本文が無くても要約行は出る
        assert!(
            msg.text.contains("requested a review"),
            "text = {}",
            msg.text
        );
    }

    /// #87: review request が通知されること。
    #[test]
    fn review_requested_notifies() {
        let msg = message("pull_request", "pull_request.review_requested.json")
            .expect("review request は通知されるべき");
        assert!(
            msg.text.contains("requested a review from <"),
            "依頼先がリンクになっていない: {}",
            msg.text
        );
        assert!(msg.text.contains("|octocat>"), "text = {}", msg.text);
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
            msg.text.contains("|team octo-team>"),
            "team がリンクになっていない: {}",
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
        assert!(
            !attach.body.text().expect("本文が無い").is_empty(),
            "本文が空"
        );
        // どのファイルへのコメントかが分かること
        assert!(attach.title.is_some(), "path が入っていない");
    }
    /// 通知対象にしていない action は `Skipped` になり、理由に action 名が入ること。
    /// `Unexpected` と一緒にすると本物の失敗がログで埋もれる。
    #[test]
    fn unsupported_action_is_skipped_with_the_action_name() {
        let err = message("pull_request_review", "pull_request_review.dismissed.json")
            .expect_err("dismissed は通知しない");

        match err {
            NotRendered::Skipped(why) => {
                assert!(why.contains("Dismissed"), "action 名が入っていない: {why}")
            }
            NotRendered::Unexpected(why) => {
                panic!("意図的なスキップが想定外扱いされている: {why}")
            }
        }
    }

    /// user も team も無い review request は想定が崩れているので `Unexpected`。
    /// こちらはログに出したい側。
    #[test]
    fn review_request_without_reviewer_is_unexpected() {
        let err = message_without(
            "pull_request",
            "pull_request.review_requested.json",
            &["requested_reviewer"],
        )
        .expect_err("user も team も無ければ通知は作れない");

        match err {
            NotRendered::Unexpected(why) => {
                assert!(
                    why.contains("requested_reviewer"),
                    "理由が分からない: {why}"
                )
            }
            NotRendered::Skipped(why) => panic!("想定外が意図的なスキップ扱い: {why}"),
        }
    }
}
