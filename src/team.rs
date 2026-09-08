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

/// ページを辿る上限。API が常に満杯のページを返しても止まるようにする。
const MAX_PAGES: usize = 20;

/// GitHub API 1 リクエストのタイムアウト。
///
/// reqwest にはデフォルトのタイムアウトが無い。API が応答しないと webhook の
/// レスポンスを返せず、GitHub 側が再送してしまうので必ず入れる
/// (fail-open にするには「有限時間で失敗する」ことが前提)。
const API_TIMEOUT: Duration = Duration::from_secs(3);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// team 展開全体に使える時間。
///
/// 1 リクエストにタイムアウトを付けても、team を直列に引く以上、合計は
/// team 数 × ページ数だけ伸びる。GitHub の webhook 配信タイムアウト (10 秒)
/// を超えると再送されるうえ、この後に Slack へ POST する時間も要るので、
/// 展開全体を短く打ち切る。
const TOTAL_EXPAND_BUDGET: Duration = Duration::from_secs(5);

/// キャッシュに載せる team の上限。
///
/// key は body に書かれた任意の文字列なので、上限が無いと存在しない team の
/// 分だけ際限なく増える。
const MAX_CACHE_ENTRIES: usize = 1024;

/// 1 つの body で展開する team の上限。
///
/// 大量の `@org/team` を書かれると、その分だけ API を直列に叩いてしまい、
/// レート制限を消費した上に webhook のレスポンスが遅れて再送を招く。
const MAX_TEAMS_PER_BODY: usize = 8;

/// 取得に失敗した team を再取得するまでの期間。
///
/// 存在しない team を毎回引き直さないようにする。成功時より短くして、
/// 一時的な失敗からは早めに復帰させる。
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
struct Member {
    login: String,
}

struct CacheEntry {
    /// 取得に失敗した場合は `None` (存在しない team を毎回引かないため)
    members: Option<Vec<String>>,
    fetched_at: Instant,
}

impl CacheEntry {
    /// まだ使えるか。失敗のキャッシュは短めに切る。
    fn is_fresh(&self) -> bool {
        let ttl = if self.members.is_some() {
            CACHE_TTL
        } else {
            NEGATIVE_CACHE_TTL
        };
        self.fetched_at.elapsed() < ttl
    }
}

#[derive(Debug)]
pub enum Error {
    /// token が設定されていないので API を叩けない
    NoToken,
    Request(reqwest::Error),
    /// 2xx 以外
    Status(reqwest::StatusCode),
    /// 直前の取得が失敗していて、まだ再取得の時期ではない
    CachedFailure,
    /// ページ上限を超えた。一部だけ返すと通知が静かに欠けるのでエラーにする
    TooManyMembers,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoToken => write!(f, "GITHUB_TOKEN is not set"),
            Self::Request(e) => write!(f, "request failed: {e}"),
            Self::Status(s) => write!(f, "unexpected status: {s}"),
            Self::CachedFailure => write!(f, "previous lookup failed (cached)"),
            Self::TooManyMembers => {
                write!(f, "team has more than {} members", PER_PAGE * MAX_PAGES)
            }
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
                .timeout(API_TIMEOUT)
                .connect_timeout(CONNECT_TIMEOUT)
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
        // token が無いときはここで諦める。イベントごとに warn と Sentry を
        // 出すと「省略可・設定しなければ静かに無効」という設計と矛盾するので、
        // 通知は起動時の warn 1 回だけにする。
        if self.token.is_none() {
            return String::new();
        }

        let teams = self.teams_in(body);
        if teams.is_empty() {
            return String::new();
        }

        let started = Instant::now();

        let mut mentions: Vec<String> = Vec::new();
        for (i, (org, slug)) in teams.iter().enumerate() {
            // 直列に引くので、全体の経過時間で打ち切る
            if started.elapsed() >= TOTAL_EXPAND_BUDGET {
                let msg = format!(
                    "team expansion budget exceeded; {} team(s) left unexpanded",
                    teams.len() - i
                );
                warn!("{msg}");
                sentry::capture_message(&msg, sentry::Level::Warning);
                break;
            }

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
                // 失敗はキャッシュしてあるので、同じ内容を Sentry に積み続けない
                Err(Error::CachedFailure) => {
                    debug!("skipping @{org}/{slug}: previous lookup failed (cached)");
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

    /// body から展開対象の team を取り出す。
    /// 同じ team を 2 回引かないよう重複を落とし、上限で打ち切る。
    fn teams_in(&self, body: &str) -> Vec<(String, String)> {
        let mut teams: Vec<(String, String)> = Vec::new();
        for cap in self.mention.captures_iter(body) {
            let team = (cap[1].to_string(), cap[2].to_string());
            if !teams.contains(&team) {
                teams.push(team);
            }
        }

        if teams.len() > MAX_TEAMS_PER_BODY {
            warn!(
                "too many team mentions ({}); expanding only the first {}",
                teams.len(),
                MAX_TEAMS_PER_BODY
            );
            teams.truncate(MAX_TEAMS_PER_BODY);
        }

        teams
    }

    /// team のメンバーの login。キャッシュがあればそれを返す。
    async fn members(&self, org: &str, slug: &str) -> Result<Vec<String>, Error> {
        let key = format!("{org}/{slug}");

        // await をまたいでロックを持たないように、スコープを切って読む
        {
            let cache = self.cache.read().expect("team cache lock poisoned");
            if let Some(entry) = cache.get(&key)
                && entry.is_fresh()
            {
                return entry.members.clone().ok_or(Error::CachedFailure);
            }
        }

        let result = self.fetch_members(org, slug).await;

        // token 未設定は team ごとの失敗ではないのでキャッシュしない
        if !matches!(result, Err(Error::NoToken)) {
            self.remember(key, result.as_ref().ok().cloned());
        }

        result
    }

    /// キャッシュに載せる。期限切れを掃除し、上限を超えていたら載せない。
    ///
    /// key は body に書かれた任意の文字列なので、掃除しないと存在しない team の
    /// 分だけプロセスの寿命だけ増え続ける。
    fn remember(&self, key: String, members: Option<Vec<String>>) {
        let mut cache = self.cache.write().expect("team cache lock poisoned");

        cache.retain(|_, entry| entry.is_fresh());

        if cache.len() >= MAX_CACHE_ENTRIES && !cache.contains_key(&key) {
            warn!("team cache is full ({MAX_CACHE_ENTRIES}); not caching {key}");
            return;
        }

        cache.insert(
            key,
            CacheEntry {
                members,
                fetched_at: Instant::now(),
            },
        );
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
            if page > MAX_PAGES {
                // 一部だけ返してキャッシュすると、載らなかった人に通知が
                // 飛ばなくなる。しかも 10 分そのままなので静かに壊れる。
                // 部分的な結果は返さず、エラーにして気づけるようにする。
                return Err(Error::TooManyMembers);
            }
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

        assert_eq!(
            r.teams_in(body),
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
        assert!(r.teams_in("@sksat をお願いします").is_empty());
    }

    /// 同じ team を何度書かれても 1 回しか引かないこと。
    #[test]
    fn duplicate_team_mentions_are_deduped() {
        let r = resolver();
        let body = "@arkedge/sat-sw @arkedge/sat-sw @arkedge/sat-sw";
        assert_eq!(
            r.teams_in(body),
            vec![("arkedge".to_string(), "sat-sw".to_string())]
        );
    }

    /// 大量に team メンションを書かれても、引く数に上限があること。
    /// 上限が無いと 1 通の webhook でレート制限を消費し、
    /// レスポンスが遅れて GitHub 側の再送を招く。
    #[test]
    fn team_mentions_are_capped() {
        let r = resolver();
        let body = (0..MAX_TEAMS_PER_BODY * 2)
            .map(|i| format!("@arkedge/team-{i}"))
            .collect::<Vec<_>>()
            .join(" ");

        assert_eq!(r.teams_in(&body).len(), MAX_TEAMS_PER_BODY);
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

    /// 期限切れのエントリが insert 時に掃除されること。
    /// key は body 由来の任意文字列なので、掃除しないと際限なく増える。
    #[test]
    fn expired_cache_entries_are_pruned() {
        let r = resolver();

        {
            let mut cache = r.cache.write().unwrap();
            cache.insert(
                "old/team".to_string(),
                CacheEntry {
                    members: Some(vec!["a".to_string()]),
                    fetched_at: Instant::now() - CACHE_TTL - Duration::from_secs(1),
                },
            );
        }

        r.remember("new/team".to_string(), Some(vec!["b".to_string()]));

        let cache = r.cache.read().unwrap();
        assert!(!cache.contains_key("old/team"), "期限切れが残っている");
        assert!(cache.contains_key("new/team"));
    }

    /// 失敗のキャッシュは成功より短い TTL で切れること。
    /// 存在しない team を毎回引かず、かつ一時的な失敗からは早く復帰させる。
    #[test]
    fn negative_cache_expires_sooner_than_positive() {
        let elapsed = NEGATIVE_CACHE_TTL + Duration::from_secs(1);

        let failed = CacheEntry {
            members: None,
            fetched_at: Instant::now() - elapsed,
        };
        assert!(!failed.is_fresh(), "失敗のキャッシュは切れているべき");

        let ok = CacheEntry {
            members: Some(vec![]),
            fetched_at: Instant::now() - elapsed,
        };
        assert!(ok.is_fresh(), "成功のキャッシュはまだ有効であるべき");
    }

    /// 展開全体の予算は GitHub の webhook 配信タイムアウト (10 秒) より
    /// 十分短くしておく。展開のあとに Slack への POST も要る。
    #[test]
    fn expand_budget_is_shorter_than_webhook_timeout() {
        assert!(TOTAL_EXPAND_BUDGET < Duration::from_secs(10));
        // 1 リクエストのタイムアウトが予算より長いと予算が意味を持たない
        assert!(API_TIMEOUT <= TOTAL_EXPAND_BUDGET);
    }

    /// token が無いときは API を叩かず、静かに展開なしで返ること。
    /// イベントごとに warn / Sentry を出さない (起動時の warn だけ)。
    #[actix_web::test]
    async fn missing_token_fails_open() {
        let r = resolver();
        assert_eq!(r.expand_mentions("@arkedge/sat-sw おねがい").await, "");
    }
}
