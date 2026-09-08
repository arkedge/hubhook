//! GitHub team のメンション (`@org/team`) をメンバーの login に展開する (#286)。
//!
//! ルールは `body` に対する正規表現なので、`@arkedge/sat-sw` と書かれても
//! `@sksat` を待っている個人のルールにはマッチせず、通知が飛ばなかった。
//! team のメンバーは payload に入っていないため GitHub API で引く。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Deserialize;
use tracing::{debug, info, warn};

/// team メンバーをキャッシュしておく期間。
/// メンバーの入れ替わりはまれなので、長めにとって API のレート制限を避ける。
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);

/// 1 ページあたりの取得件数 (GitHub API の最大値)。
const PER_PAGE: usize = 100;

/// 取得するメンバー数の上限。これを **超える** team はエラーにする。
///
/// ページ数で数えると、最終ページが満杯だった時点では「上限を超えている」と
/// 断定できず、ちょうど上限ぴったりの team を誤って弾いてしまう
/// (満杯の次が空ページかどうかは、引かないと分からない)。
/// 実際に集まった人数で判定すれば境界を正しく扱える。
const MAX_MEMBERS: usize = 2000;

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
    /// メンバー数の上限を超えた。一部だけ返すと通知が静かに欠けるのでエラーにする
    TooManyMembers,
    /// 展開に使える時間を使い切った
    BudgetExceeded,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoToken => write!(f, "GITHUB_TOKEN is not set"),
            Self::Request(e) => write!(f, "request failed: {e}"),
            Self::Status(s) => write!(f, "unexpected status: {s}"),
            Self::CachedFailure => write!(f, "previous lookup failed (cached)"),
            Self::TooManyMembers => write!(f, "team has more than {MAX_MEMBERS} members"),
            Self::BudgetExceeded => write!(f, "expansion budget exceeded"),
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
    /// GitHub API の base URL。テストで差し替える。
    base_url: String,
    /// team ごとの取得中ロック。
    ///
    /// キャッシュが空の瞬間に同じ team のリクエストが同時に来ると、全員が
    /// キャッシュミスして各自 API を叩く (cache stampede)。キャッシュだけでは
    /// 防げないので、team ごとにロックを取って取得を 1 本にまとめる。
    inflight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// `@org/team` を拾う。org / team slug に使える文字は英数と `-`、
    /// team slug には `_` と `.` も入りうる。
    mention: Regex,
    cache: RwLock<HashMap<String, CacheEntry>>,
}

impl TeamResolver {
    pub fn new(token: Option<String>) -> Self {
        Self::with_base_url(token, "https://api.github.com".to_string())
    }

    fn with_base_url(token: Option<String>, base_url: String) -> Self {
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
            base_url,
            inflight: Mutex::new(HashMap::new()),
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

        let deadline = Instant::now() + TOTAL_EXPAND_BUDGET;

        let mut mentions: Vec<String> = Vec::new();
        for (i, (org, slug)) in teams.iter().enumerate() {
            // 直列に引くので、全体の残り時間で打ち切る
            if Instant::now() >= deadline {
                let msg = format!(
                    "team expansion budget exceeded; {} team(s) left unexpanded",
                    teams.len() - i
                );
                warn!("{msg}");
                sentry::capture_message(&msg, sentry::Level::Warning);
                break;
            }

            match self.members(org, slug, deadline).await {
                Ok(members) => {
                    debug!("expanded @{org}/{slug} to {} member(s)", members.len());
                    // 重複判定を contains でやると人数の 2 乗になる
                    // (8 team × 2000 人で 1 億回規模の比較)。あとで一括で潰す。
                    mentions.extend(members.into_iter().map(|m| format!("@{m}")));
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

        // team 間で重複する人を潰す。順序は照合結果に影響しないが、
        // テストが安定するように sort してから dedup する。
        mentions.sort_unstable();
        mentions.dedup();

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
    async fn members(
        &self,
        org: &str,
        slug: &str,
        deadline: Instant,
    ) -> Result<Vec<String>, Error> {
        let key = format!("{org}/{slug}");

        // 速い経路。await をまたいでロックを持たないよう、スコープを切って読む
        if let Some(cached) = self.cached(&key) {
            return cached;
        }

        // この team の取得権を取る。同じ team を同時に引かないようにする
        let lock = self.inflight_lock(&key);
        let _guard = lock.lock().await;

        // 待っている間に、先に取得した人がキャッシュを埋めているかもしれない
        if let Some(cached) = self.cached(&key) {
            return cached;
        }

        let result = self.fetch_members(org, slug, deadline).await;

        // token 未設定は team ごとの失敗ではないのでキャッシュしない
        if !matches!(result, Err(Error::NoToken)) {
            self.remember(key, result.as_ref().ok().cloned());
        }

        result
    }

    /// キャッシュに使える値があればそれを返す。
    fn cached(&self, key: &str) -> Option<Result<Vec<String>, Error>> {
        let cache = self.cache.read().expect("team cache lock poisoned");
        let entry = cache.get(key)?;
        if !entry.is_fresh() {
            return None;
        }
        Some(entry.members.clone().ok_or(Error::CachedFailure))
    }

    /// team ごとの取得中ロックを取り出す (無ければ作る)。
    ///
    /// 使い終わったものは、他に持っている人がいなければ捨てる。
    /// key は body 由来の任意文字列なので、放置すると増え続ける。
    fn inflight_lock(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut inflight = self.inflight.lock().expect("inflight lock poisoned");

        inflight.retain(|_, lock| Arc::strong_count(lock) > 1);

        inflight
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
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

    async fn fetch_members(
        &self,
        org: &str,
        slug: &str,
        deadline: Instant,
    ) -> Result<Vec<String>, Error> {
        let token = self.token.as_deref().ok_or(Error::NoToken)?;

        let mut members = Vec::new();
        let mut page = 1;

        // メンバーが PER_PAGE を超える team もあるので、最後のページまで辿る。
        // 途中で打ち切ると、その人には通知が飛ばなくなる。
        loop {
            // 1 リクエストごとのタイムアウトだけでは、ページ数だけ合計が伸びる。
            // ページを進める前に全体の残り時間を見る。
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::BudgetExceeded);
            }

            // 残り時間より長いタイムアウトを許すと、チェックを通った直後の
            // リクエストが満額まで走って予算を超える (残り 0.1 秒で 3 秒走る)。
            // リクエスト単位のタイムアウトを残り時間で切る。
            let timeout = remaining.min(API_TIMEOUT);

            let url = format!(
                "{base}/orgs/{org}/teams/{slug}/members?per_page={PER_PAGE}&page={page}",
                base = self.base_url
            );

            let res = self
                .client
                .get(&url)
                .bearer_auth(token)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .timeout(timeout)
                .send()
                .await?;

            let status = res.status();
            if !status.is_success() {
                return Err(Error::Status(status));
            }

            let batch: Vec<Member> = res.json().await?;
            let n = batch.len();
            members.extend(batch.into_iter().map(|m| m.login));

            // 一部だけ返してキャッシュすると、載らなかった人に通知が飛ばず、
            // しかも 10 分そのままなので静かに壊れる。
            // 部分的な結果は返さず、エラーにして気付けるようにする。
            if members.len() > MAX_MEMBERS {
                return Err(Error::TooManyMembers);
            }

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

    /// `GET /orgs/{org}/teams/{slug}/members` を返すテスト用サーバを立て、
    /// base URL を返す。`page_sizes` は各ページで返す件数。
    ///
    /// HTTP パスを実際に通さないと、pagination や非 2xx 時の fail-open が
    /// 壊れても気付けない (実際どちらも一度壊している)。
    fn spawn_api(page_sizes: Vec<usize>, status: u16) -> String {
        spawn_api_with_delay(page_sizes, status, Duration::ZERO)
    }

    fn spawn_api_with_delay(page_sizes: Vec<usize>, status: u16, delay: Duration) -> String {
        spawn_api_counting(page_sizes, status, delay).0
    }

    /// 受けたリクエスト数を数えるテスト用サーバ。
    fn spawn_api_counting(
        page_sizes: Vec<usize>,
        status: u16,
        delay: Duration,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use actix_web::{App, HttpResponse, HttpServer, web};
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let sizes = Arc::new(page_sizes);
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_srv = hits.clone();

        let srv = HttpServer::new(move || {
            let sizes = sizes.clone();
            let hits = hits_srv.clone();
            App::new().route(
                "/orgs/{org}/teams/{slug}/members",
                web::get().to(move |q: web::Query<HashMap<String, String>>| {
                    let sizes = sizes.clone();
                    let hits = hits.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);

                        if !delay.is_zero() {
                            actix_web::rt::time::sleep(delay).await;
                        }

                        if status != 200 {
                            return HttpResponse::build(
                                actix_web::http::StatusCode::from_u16(status).unwrap(),
                            )
                            .finish();
                        }

                        let page: usize = q.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
                        let n = sizes.get(page - 1).copied().unwrap_or(0);
                        let body: Vec<serde_json::Value> = (0..n)
                            .map(|i| serde_json::json!({ "login": format!("u{page}_{i}") }))
                            .collect();

                        HttpResponse::Ok().json(body)
                    }
                }),
            )
        })
        .bind("127.0.0.1:0")
        .expect("could not bind test server");

        let addr = srv.addrs()[0];
        actix_web::rt::spawn(srv.run());

        (format!("http://{addr}"), hits)
    }

    fn api_resolver(base: String) -> TeamResolver {
        TeamResolver::with_base_url(Some("dummy-token".to_string()), base)
    }

    fn far_deadline() -> Instant {
        Instant::now() + Duration::from_secs(30)
    }

    /// 最後の (満杯でない) ページまで辿ること。
    #[actix_web::test]
    async fn paginates_until_short_page() {
        let r = api_resolver(spawn_api(vec![PER_PAGE, PER_PAGE, 7], 200));
        let members = r
            .members("arkedge", "sat-sw", far_deadline())
            .await
            .expect("取得できるべき");

        assert_eq!(members.len(), PER_PAGE * 2 + 7);
    }

    /// ちょうど上限ぴったりの team は受け入れること。
    ///
    /// ページ数で判定していた頃は、満杯のページが続いた時点で
    /// 「上限超え」と誤判定して弾いていた。
    #[actix_web::test]
    async fn exactly_max_members_is_accepted() {
        let mut sizes = vec![PER_PAGE; MAX_MEMBERS / PER_PAGE];
        sizes.push(0); // 満杯の次は空ページ
        let r = api_resolver(spawn_api(sizes, 200));

        let members = r
            .members("arkedge", "sat-sw", far_deadline())
            .await
            .expect("ちょうど上限なら受け入れるべき");

        assert_eq!(members.len(), MAX_MEMBERS);
    }

    /// 上限を超える team はエラーにすること (一部だけ返さない)。
    #[actix_web::test]
    async fn more_than_max_members_is_an_error() {
        let sizes = vec![PER_PAGE; MAX_MEMBERS / PER_PAGE + 1];
        let r = api_resolver(spawn_api(sizes, 200));

        let err = r
            .members("arkedge", "sat-sw", far_deadline())
            .await
            .expect_err("上限超えはエラーにするべき");

        assert!(matches!(err, Error::TooManyMembers), "{err}");
    }

    /// 非 2xx のときは展開せずに空文字を返すこと (fail-open)。
    #[actix_web::test]
    async fn non_success_status_fails_open() {
        let r = api_resolver(spawn_api(vec![], 403));
        assert_eq!(r.expand_mentions("@arkedge/sat-sw おねがい").await, "");
    }

    /// 取得できた team メンバーが @login として展開されること。
    #[actix_web::test]
    async fn members_are_expanded_as_mentions() {
        let r = api_resolver(spawn_api(vec![2], 200));
        let expanded = r.expand_mentions("@arkedge/sat-sw おねがい").await;

        assert_eq!(expanded, "@u1_0 @u1_1");
    }

    /// 予算を使い切っていたら API を叩かずエラーにすること。
    #[actix_web::test]
    async fn exhausted_budget_stops_before_request() {
        let r = api_resolver(spawn_api(vec![1], 200));
        let past = Instant::now() - Duration::from_secs(1);

        let err = r
            .members("arkedge", "sat-sw", past)
            .await
            .expect_err("予算切れならエラーにするべき");

        assert!(matches!(err, Error::BudgetExceeded), "{err}");
    }

    /// 残り予算が短いときは、リクエスト単位のタイムアウト (3 秒) を
    /// 待たずに打ち切ること。
    ///
    /// 予算チェックを通った直後のリクエストが満額まで走ると、
    /// 5 秒の予算を超えて webhook のレスポンスが遅れる。
    #[actix_web::test]
    async fn request_is_bounded_by_remaining_budget() {
        // 応答しないサーバ (API_TIMEOUT より長く待たせる)
        let base = spawn_api_with_delay(vec![1], 200, API_TIMEOUT * 2);
        let r = api_resolver(base);

        let started = Instant::now();
        let deadline = started + Duration::from_millis(500);
        let result = r.members("arkedge", "sat-sw", deadline).await;
        let elapsed = started.elapsed();

        assert!(result.is_err(), "予算内に返らないのでエラーになるべき");
        // 修正前は API_TIMEOUT (3 秒) まで走る。0.5 秒と 3 秒を余裕をもって分ける
        assert!(
            elapsed < Duration::from_secs(2),
            "残り予算 (0.5 秒) ではなく API_TIMEOUT ({API_TIMEOUT:?}) まで待っている: {elapsed:?}"
        );
    }

    /// 同じ team に同時にリクエストが来ても、API は 1 回しか叩かないこと。
    ///
    /// キャッシュが空の瞬間は全員がミスするので、キャッシュだけでは防げない
    /// (cache stampede)。team ごとのロックで 1 本にまとめている。
    #[actix_web::test]
    async fn concurrent_lookups_share_one_request() {
        use std::sync::atomic::Ordering;

        // 全員がキャッシュミスを踏めるよう、応答を少し遅らせる
        let (base, hits) = spawn_api_counting(vec![2], 200, Duration::from_millis(200));
        let r = Arc::new(api_resolver(base));

        let mut tasks = Vec::new();
        for _ in 0..3 {
            let r = r.clone();
            tasks.push(actix_web::rt::spawn(async move {
                r.members("arkedge", "sat-sw", far_deadline()).await
            }));
        }

        for t in tasks {
            let members = t.await.expect("task panicked").expect("取得できるべき");
            assert_eq!(members.len(), 2);
        }

        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "同じ team を複数回引いている"
        );
    }

    /// 2 回目は API を叩かずキャッシュから返すこと。
    #[actix_web::test]
    async fn second_lookup_hits_cache() {
        let r = api_resolver(spawn_api(vec![3], 200));

        let first = r
            .members("arkedge", "sat-sw", far_deadline())
            .await
            .unwrap();
        assert_eq!(first.len(), 3);

        // キャッシュに載っているので、予算切れでも返る
        let past = Instant::now() - Duration::from_secs(1);
        let second = r.members("arkedge", "sat-sw", past).await.unwrap();
        assert_eq!(second, first);
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
