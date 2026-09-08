//! GitHub team のメンション (`@org/team`) をメンバーの login に展開する (#286)。
//!
//! ルールは `body` に対する正規表現なので、`@arkedge/sat-sw` と書かれても
//! `@sksat` を待っている個人のルールにはマッチせず、通知が飛ばなかった。
//! team のメンバーは payload に入っていないため GitHub API で引く。

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Deserialize;
use tracing::{debug, info, warn};

/// team メンバーをキャッシュしておく期間。
/// メンバーの入れ替わりはまれなので、長めにとって API のレート制限を避ける。
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);

/// 1 ページあたりの取得件数 (GitHub API の最大値)。
const PER_PAGE: usize = 100;

#[derive(Debug, Deserialize)]
struct Member {
    login: String,
}

struct CacheEntry {
    members: Vec<String>,
    fetched_at: Instant,
}

#[derive(Debug)]
pub enum Error {
    /// token が設定されていないので API を叩けない
    NoToken,
    Request(reqwest::Error),
    /// 2xx 以外
    Status(reqwest::StatusCode),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoToken => write!(f, "GITHUB_TOKEN is not set"),
            Self::Request(e) => write!(f, "request failed: {e}"),
            Self::Status(s) => write!(f, "unexpected status: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Self::Request(e)
    }
}

pub struct TeamResolver {
    client: reqwest::Client,
    token: Option<String>,
    /// `@org/team` を拾う。org / team slug に使える文字は英数と `-`、
    /// team slug には `_` と `.` も入りうる。
    mention: Regex,
    cache: RwLock<HashMap<String, CacheEntry>>,
}

impl TeamResolver {
    pub fn new(token: Option<String>) -> Self {
        // docker-compose などで `GITHUB_TOKEN=${GITHUB_TOKEN}` と書くと、
        // 未設定でも空文字が入って Some("") になる。空 token で API を叩いても
        // 401 になるだけなので、未設定として扱う。
        let token = token.filter(|t| !t.is_empty());

        if token.is_none() {
            warn!("GITHUB_TOKEN is not set: team mentions will not be expanded (#286)");
        }

        Self {
            client: reqwest::Client::builder()
                // GitHub API は User-Agent が無いと 403 を返す
                .user_agent("hubhook")
                .build()
                .expect("could not build http client"),
            token,
            mention: Regex::new(r"@([A-Za-z0-9][A-Za-z0-9-]*)/([A-Za-z0-9][A-Za-z0-9._-]*)")
                .expect("invalid team mention regex"),
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// `body` 中の team メンションをメンバーの `@login` に展開し、
    /// スペース区切りで返す。team メンションが無ければ空文字。
    ///
    /// 展開に失敗しても、他のルールの判定は続けたいので空文字を返す
    /// (fail-open)。失敗は log と sentry に出す。
    pub async fn expand_mentions(&self, body: &str) -> String {
        // 同じ team を 2 回引かないように、先に重複を落とす
        let mut teams: Vec<(String, String)> = Vec::new();
        for cap in self.mention.captures_iter(body) {
            let team = (cap[1].to_string(), cap[2].to_string());
            if !teams.contains(&team) {
                teams.push(team);
            }
        }

        if teams.is_empty() {
            return String::new();
        }

        let mut mentions: Vec<String> = Vec::new();
        for (org, slug) in &teams {
            match self.members(org, slug).await {
                Ok(members) => {
                    debug!("expanded @{org}/{slug} to {} member(s)", members.len());
                    for m in members {
                        let mention = format!("@{m}");
                        if !mentions.contains(&mention) {
                            mentions.push(mention);
                        }
                    }
                }
                Err(e) => {
                    // 展開できなくても、team メンション以外のルールは動かしたい
                    let msg = format!("could not expand team mention @{org}/{slug}: {e}");
                    warn!("{msg}");
                    sentry::capture_message(&msg, sentry::Level::Warning);
                }
            }
        }

        mentions.join(" ")
    }

    /// team のメンバーの login。キャッシュがあればそれを返す。
    async fn members(&self, org: &str, slug: &str) -> Result<Vec<String>, Error> {
        let key = format!("{org}/{slug}");

        // await をまたいでロックを持たないように、スコープを切って読む
        {
            let cache = self.cache.read().expect("team cache lock poisoned");
            if let Some(entry) = cache.get(&key)
                && entry.fetched_at.elapsed() < CACHE_TTL
            {
                return Ok(entry.members.clone());
            }
        }

        let members = self.fetch_members(org, slug).await?;

        {
            let mut cache = self.cache.write().expect("team cache lock poisoned");
            cache.insert(
                key,
                CacheEntry {
                    members: members.clone(),
                    fetched_at: Instant::now(),
                },
            );
        }

        Ok(members)
    }

    async fn fetch_members(&self, org: &str, slug: &str) -> Result<Vec<String>, Error> {
        let token = self.token.as_deref().ok_or(Error::NoToken)?;

        let mut members = Vec::new();
        let mut page = 1;

        // メンバーが PER_PAGE を超える team もあるので、最後のページまで辿る。
        // 途中で打ち切ると、その人には通知が飛ばなくなる。
        loop {
            let url = format!(
                "https://api.github.com/orgs/{org}/teams/{slug}/members\
                 ?per_page={PER_PAGE}&page={page}"
            );

            let res = self
                .client
                .get(&url)
                .bearer_auth(token)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .send()
                .await?;

            let status = res.status();
            if !status.is_success() {
                return Err(Error::Status(status));
            }

            let batch: Vec<Member> = res.json().await?;
            let n = batch.len();
            members.extend(batch.into_iter().map(|m| m.login));

            if n < PER_PAGE {
                break;
            }
            page += 1;
        }

        info!("fetched {} member(s) of @{org}/{slug}", members.len());

        Ok(members)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> TeamResolver {
        TeamResolver::new(None)
    }

    /// body から team メンションだけを拾えること。
    #[test]
    fn mention_regex_picks_up_teams() {
        let r = resolver();
        let body = "@sksat @arkedge/sat-sw をお願いします。@arkedge/infra も。";
        let found: Vec<(String, String)> = r
            .mention
            .captures_iter(body)
            .map(|c| (c[1].to_string(), c[2].to_string()))
            .collect();

        assert_eq!(
            found,
            vec![
                ("arkedge".to_string(), "sat-sw".to_string()),
                ("arkedge".to_string(), "infra".to_string()),
            ]
        );
    }

    /// 個人のメンションを team として拾ってしまわないこと。
    #[test]
    fn plain_user_mention_is_not_a_team() {
        let r = resolver();
        assert!(r.mention.captures("@sksat をお願いします").is_none());
    }

    /// team メンションが無ければ API を叩かずに空文字を返すこと
    /// (token 未設定でもここは動く)。
    #[actix_web::test]
    async fn no_team_mention_expands_to_empty() {
        let r = resolver();
        assert_eq!(r.expand_mentions("@sksat おねがい").await, "");
        assert_eq!(r.expand_mentions("").await, "");
    }

    /// 空文字の token は未設定として扱うこと。
    /// docker-compose の `${GITHUB_TOKEN}` が未設定だとこうなる。
    #[test]
    fn empty_token_is_treated_as_unset() {
        let r = TeamResolver::new(Some(String::new()));
        assert!(r.token.is_none());
    }

    /// token が無いときは展開できないが、panic せず空文字で返ること。
    #[actix_web::test]
    async fn missing_token_fails_open() {
        let r = resolver();
        assert_eq!(r.expand_mentions("@arkedge/sat-sw おねがい").await, "");
    }
}
