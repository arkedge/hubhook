use std::pin::Pin;
use std::sync::Arc;

use structopt::StructOpt;

use serde::Deserialize;

use regex::{Regex, RegexBuilder};

use actix_web::error::ErrorBadRequest;
use actix_web::{App, Error, FromRequest, HttpRequest, HttpResponse, HttpServer, Result, web};

use futures::future::{Future, FutureExt};
use futures::stream::TryStreamExt;

use tracing::{debug, error, info, warn};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

mod github;
mod message;
mod slack;
mod team;

#[derive(Debug, Clone, StructOpt)]
#[structopt(name = "hubhook")]
struct Opt {
    #[structopt(env, default_value = "/config/config.json", long, short)]
    config_path: String,

    #[structopt(long, env)]
    hubhook_port: usize,
    #[structopt(long, env)]
    slack_token: String,
    #[structopt(long, env)]
    webhook_secret: String,

    #[structopt(long, env)]
    sentry_dsn: String,

    /// team メンションを展開するための GitHub token (#286)。
    /// 未設定でも動くが、team メンションは展開されない。
    #[structopt(long, env)]
    github_token: Option<String>,

    #[structopt(long)]
    debug: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Config {
    pub rule: Vec<Rule>,
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct RuleMatchResult {
    display_name: String,
    channel: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub channel: String,
    pub query: Query,
    pub exclude_query: Option<Query>,
    pub display_name: String,
}

// TODO: empty check
#[derive(Debug, Clone, Deserialize)]
pub struct Query {
    repo: Option<String>,
    topic: Option<String>,
    user: Option<String>,
    //event: Option<String>,
    title: Option<String>,
    body: Option<String>,
    label: Option<String>,
    /// Issue / PR の assignee の login (#41)
    assignee: Option<String>,
    /// review を依頼された user の login、または team の slug (#87)
    reviewer: Option<String>,
    /// `pull_request_review` の state (`approved` / `changes_requested` / `commented`)。
    /// review 以外のイベントに対しては常に不一致になる (#285)。
    review_state: Option<String>,
}

#[derive(Debug)]
struct Data {
    /// 扱わないイベントは `None`
    payload: Option<github::Payload>,
}

impl FromRequest for Data {
    type Error = Error;
    //type Future = Ready<Result<Self, Self::Error>>;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, payload: &mut actix_web::dev::Payload) -> Self::Future {
        use futures::future::err;

        debug!("{:?}", req);

        let headers = req.headers();
        let ua = headers.get("user-agent").unwrap();
        let ua_str = ua.to_str().unwrap();
        if !ua_str.starts_with("GitHub-Hookshot") {
            error!("user-agent mismatch");
            return Box::pin(err(ErrorBadRequest("user-agent mismatch")));
        }

        let event = match headers.get("x-github-event").and_then(|e| e.to_str().ok()) {
            Some(event) => event.to_string(),
            None => {
                error!("missing X-GitHub-Event header");
                return Box::pin(err(ErrorBadRequest("missing X-GitHub-Event header")));
            }
        };

        let sig256: Vec<u8> = {
            let sig = headers.get("x-hub-signature-256").unwrap();
            let sig = String::from_utf8(sig.as_bytes().to_vec()).unwrap();
            let sig = sig.strip_prefix("sha256=").unwrap();
            hex::decode(sig).unwrap()
        };

        let req = req.clone();
        let pd = payload.take();
        async move {
            let opt = req.app_data::<web::Data<Arc<Opt>>>().unwrap();
            let p = pd
                .try_fold(Vec::new(), |mut acc, chunk| async move {
                    acc.extend(chunk);
                    Ok(acc)
                })
                .await;
            let p: Vec<u8> = p.unwrap();

            // validate signature
            if !verify_signature(opt.webhook_secret.as_bytes(), &p, &sig256) {
                error!("signature mismatch");
                if !opt.debug {
                    return Err(ErrorBadRequest("signature mismatch!"));
                }
            }

            let payload = match github::Payload::from_event(&event, &p) {
                Ok(payload) => payload,
                Err(e) => {
                    // untagged をやめたので、どのフィールドで失敗したかがそのまま出る
                    let msg = format!("could not deserialize {event} payload: {e}");
                    error!("{msg}");
                    sentry::capture_message(&msg, sentry::Level::Error);
                    return Err(ErrorBadRequest("could not deserialize payload"));
                }
            };

            if payload.is_none() {
                debug!("ignoring event: {event}");
            }

            Ok(Data { payload }) // validate success
        }
        .boxed_local()
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // SAFETY: actix のランタイムはカレントスレッド上で動いており、この時点では
    // まだワーカースレッドも sentry の転送スレッドも生成されていないため、
    // 環境変数の書き込みが他スレッドの読み取りと競合することはない。
    // sentry::init はスレッドを立てるので、必ずそれより前に置くこと。
    unsafe { std::env::set_var("RUST_BACKTRACE", "1") };

    let opt = Opt::from_args();

    // sentry 0.49 で ClientOptions が #[non_exhaustive] になり、
    // 構造体リテラル + `..Default::default()` では作れなくなった (E0639)。
    // 代わりに用意された builder を使う。release_name!() は Option を返すので
    // maybe_release を使うのが本家推奨。
    let _guard = sentry::init((
        opt.sentry_dsn.clone(),
        sentry::ClientOptions::new().maybe_release(sentry::release_name!()),
    ));

    let port = opt.hubhook_port;

    // レベルを固定すると、後から「なぜ通知が飛ばなかったか」を追うために
    // 再起動が要る。RUST_LOG で上書きできるようにしておく。
    // 既定は info。warn 止めだと通知が飛んだこと自体が残らない。
    let default = if opt.debug { "debug" } else { "info" };
    let filter = match std::env::var("RUST_LOG") {
        // 壊れた RUST_LOG で黙って既定に落ちると、レベルを変えたつもりで
        // 変わっていないことに気付けない。まだ subscriber が無いので stderr に出す。
        // docker-compose で `RUST_LOG=${RUST_LOG}` と書くと、未設定でも空文字が
        // 入って Some("") になる。空の EnvFilter は directive を持たないので
        // 何も出なくなってしまう。未設定として扱う。
        Ok(spec) if spec.is_empty() => tracing_subscriber::EnvFilter::new(default),
        Ok(spec) => tracing_subscriber::EnvFilter::try_new(&spec).unwrap_or_else(|e| {
            eprintln!("invalid RUST_LOG ({spec:?}), falling back to {default}: {e}");
            tracing_subscriber::EnvFilter::new(default)
        }),
        // 未設定は既定でよい。設定されているのに読めない (NotUnicode) のは
        // 設定ミスなので、黙って未設定と同じ扱いにしない。
        Err(std::env::VarError::NotPresent) => tracing_subscriber::EnvFilter::new(default),
        Err(e) => {
            eprintln!("could not read RUST_LOG, falling back to {default}: {e}");
            tracing_subscriber::EnvFilter::new(default)
        }
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cfg: Config = {
        use std::io::Read;

        info!("loading config file from \"{}\"", opt.config_path);
        let f = std::fs::File::open(opt.config_path.clone());
        if let Err(ref _f) = f {
            error!("could not open config file!");
        }

        let mut f = f.unwrap();
        let mut config = String::new();

        if f.read_to_string(&mut config).is_err() {
            error!("could not read config file");
        }

        let res = serde_json::from_str(&config);
        if let Err(ref e) = res {
            error!("could not deserialize config file!");
            error!("{}", e);
        }
        res.unwrap()
    };

    // キャッシュを worker 間で共有するため、closure の外で 1 つだけ作る
    let teams = Arc::new(team::TeamResolver::new(opt.github_token.clone()));

    HttpServer::new(move || {
        App::new()
            .wrap(sentry_actix::Sentry::new())
            .app_data(web::Data::new(Arc::new(cfg.clone()))) // memo: https://github.com/actix/actix-web/issues/1454#issuecomment-867897725
            .app_data(web::Data::new(Arc::new(opt.clone())))
            .app_data(web::Data::new(teams.clone()))
            .service(web::resource("/webhook").route(web::post().to(webhook)))
            .service(web::resource("/healthcheck").route(web::get().to(HttpResponse::Ok)))
    })
    .bind(format!("0.0.0.0:{}", port))?
    .run()
    .await
}

async fn webhook(
    opt: web::Data<Arc<Opt>>,
    cfg: web::Data<Arc<Config>>,
    teams: web::Data<Arc<team::TeamResolver>>,
    data: Data,
) -> Result<HttpResponse> {
    // 扱わないイベントは何もしない
    let Some(payload) = data.payload else {
        return Ok(HttpResponse::Ok().body("ignored"));
    };

    //post_test(&opt, &payload).await;

    // team メンションをメンバーの @login に展開してから照合する (#286)。
    // body クエリを使う rule が 1 つも無ければ展開結果は使われないので、
    // GitHub API を叩かない (repo / label / assignee だけの設定で待たされないため)。
    let extra_mentions = if cfg.rule.iter().any(|r| r.uses_body()) {
        teams.expand_mentions(payload.body()).await
    } else {
        String::new()
    };

    // match rule
    let matches = payload.match_rules(&cfg.rule, &extra_mentions);

    if matches.is_empty() {
        return Ok(HttpResponse::Ok().body("webhook"));
    }

    // 変換できるかは payload だけで決まるので、rule が複数当たっても失敗の理由は
    // 同じ。ログは 1 回で足りる。
    let rendered: Result<slack::Message, _> = (&payload).try_into();
    let mut first = match rendered {
        Ok(msg) => Some(msg),
        Err(why) => {
            match &why {
                // 通知対象にしていない action。error! で出すと本物の失敗が
                // 埋もれるが、debug だと「なぜ飛ばなかったか」を後から追えない。
                message::NotRendered::Skipped(_) => {
                    info!(reason = %why, link = %payload.url(), "not notified");
                }
                message::NotRendered::Unexpected(_) => {
                    error!(
                        reason = %why,
                        link = %payload.url(),
                        "GitHub payload -> slack::Message failed"
                    );
                }
            }
            return Ok(HttpResponse::Ok().body("webhook"));
        }
    };

    for (channel, m) in matches {
        // post_message が self を取るので channel ごとに作り直すが、1 通目は
        // 上で作ったものを使い回す。
        let msg = match first.take() {
            Some(msg) => msg,
            None => {
                let msg: Result<slack::Message, _> = (&payload).try_into();
                // 上で変換できることを確認済み
                let Ok(msg) = msg else { continue };
                msg
            }
        };
        msg.post_message(
            &opt.slack_token,
            &channel,
            Some(&m.display_name),
            payload.url().as_str(),
        )
        .await;
    }

    Ok(HttpResponse::Ok().body("webhook"))
}

impl Rule {
    /// body クエリを使っているか (include / exclude のいずれか)。
    ///
    /// 使っていない rule しか無いなら team を引く必要がない。
    fn uses_body(&self) -> bool {
        self.query.body.is_some()
            || self
                .exclude_query
                .as_ref()
                .is_some_and(|q| q.body.is_some())
    }

    /// `mentions` は team メンションを展開した `@login` の列、
    /// `combined` は「元の本文 + `mentions`」を組み立てたもの (#286)。
    /// どちらも webhook ごとに 1 回作って rule 間で使い回す。
    fn check_match(&self, payload: &github::Payload, mentions: &str, combined: &str) -> bool {
        let include_query_result = Rule::match_results(&self.query, payload, mentions, combined)
            .iter()
            .all(|&r| r);

        if let Some(exclude_query) = &self.exclude_query {
            let exclude_query_result =
                Rule::match_results(exclude_query, payload, mentions, combined)
                    .iter()
                    .any(|&r| r);
            include_query_result && !exclude_query_result
        } else {
            include_query_result
        }
    }

    fn match_results(
        query: &Query,
        payload: &github::Payload,
        mentions: &str,
        combined: &str,
    ) -> Vec<bool> {
        let r_repo = Rule::match_query(query.repo.as_ref(), &payload.repo().full_name);

        let topics = &payload.repo().topics;
        let topics = topics.iter().collect();
        let r_topic = Rule::match_query_vec(query.topic.as_ref(), topics);

        let r_sender = Rule::match_query(query.user.as_ref(), &payload.sender().login);
        let r_title = Rule::match_query(query.title.as_ref(), payload.title());
        // body クエリは 3 つの対象に当てて OR を取る。正規表現のコンパイルは 1 回。
        let r_body = query.body.as_ref().map(|q| {
            let Some(re) = Rule::compile_query(q) else {
                return false;
            };
            let body = payload.body();

            // 1. 元の本文。`@org/team$` のようなアンカー付きルールの意味を保つ。
            //    連結したものだけに当てると末尾一致が効かなくなり、
            //    exclude_query 側では除外されるべきものが除外されなくなる。
            if re.is_match(body) {
                return true;
            }

            if mentions.is_empty() {
                return false;
            }

            // 2. 元の本文 + 展開結果。本文の文脈と組み合わせたパターン
            //    (`レビュー.*@sksat` など) を拾う。区切りは改行ではなく空白
            //    (正規表現の `.` は既定で改行に一致しない)。
            //    組み立て済みのものを受け取るので、rule ごとには確保しない。
            if re.is_match(combined) {
                return true;
            }

            // 3. 展開された `@login` を 1 つずつ。まとめて 1 つの文字列に当てると、
            //    `@sksat$` のようなアンカー付きルールが「たまたま最後に並んだか」で
            //    結果が変わってしまう (並び順は展開側の都合に過ぎない)。
            //
            //    ここで本文を連結しないのは、rule ごと × member ごとに本文長を
            //    走査することになり、rule が増えるほど webhook 1 通の処理が
            //    重くなるため。その結果、「本文の文脈 + member への末尾アンカー」
            //    (`レビュー.*@sksat$`) は並び順に依存するという制限が残る。
            //    稀な書き方のために全体のコストを上げない判断 (README に記載)。
            mentions.split(' ').any(|m| re.is_match(m))
        });

        let labels = payload.labels().iter().collect();
        let r_labels = Rule::match_query_vec(query.label.as_ref(), labels);

        let assignees = payload.assignees().iter().collect();
        let r_assignee = Rule::match_query_vec(query.assignee.as_ref(), assignees);

        // review を依頼されたイベント以外では空なので、reviewer を指定した rule は
        // review_requested にしかマッチしない
        let r_reviewer =
            Rule::match_query_vec(query.reviewer.as_ref(), payload.requested_reviewers());

        // review_state を持たないイベントには、query が指定されていれば必ず不一致を返す。
        // 空文字を照合対象にすると、query は正規表現なので `.*` や `^$` のような
        // パターンがマッチしてしまい、review 以外のイベントまで通知されてしまう
        // (exclude_query 側では逆に、意図しない抑制になる)。
        let r_review_state = match (query.review_state.as_ref(), payload.review_state()) {
            (None, _) => None,
            (Some(_), None) => Some(false),
            (Some(query), Some(state)) => Some(Rule::match_query_impl(query, state)),
        };

        vec![
            r_repo,
            r_topic,
            r_sender,
            r_title,
            r_body,
            r_labels,
            r_assignee,
            r_reviewer,
            r_review_state,
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn match_query(query: Option<&String>, payload: &str) -> Option<bool> {
        query?;
        Some(Rule::match_query_impl(query.unwrap(), payload))
    }

    fn match_query_vec<T>(query: Option<&String>, payload: Vec<T>) -> Option<bool>
    where
        T: ToString, // Into<&str>にしようとしたけどダメだった
    {
        query?;
        let query = query.unwrap();

        for p in payload {
            if Rule::match_query_impl(query, &p.to_string()) {
                return Some(true);
            }
        }

        Some(false)
    }

    /// query を正規表現にする。空クエリは警告して `None`。
    fn compile_query(query: &str) -> Option<Regex> {
        if query.is_empty() {
            warn!("query is empty");
            return None;
        }

        Some(
            RegexBuilder::new(query)
                .case_insensitive(true)
                .build()
                .unwrap(),
        )
    }

    fn match_query_impl(query: &str, payload: &str) -> bool {
        Rule::compile_query(query).is_some_and(|re| re.is_match(payload))
    }
}

#[allow(dead_code)]
async fn post_test(opt: &Opt, payload: &github::Payload) {
    let msg: slack::Message = payload.try_into().unwrap();
    msg.post_message(
        &opt.slack_token,
        "tmp_hubhook",
        None,
        payload.url().as_str(),
    )
    .await;
}

/// webhook の署名 (`X-Hub-Signature-256`) を検証する。
///
/// 比較は hmac の [`Mac::verify_slice`] に任せる。定数時間で比較されるので、
/// 一致する先頭バイト数が処理時間に出ない。自前で 1 バイトずつ比較して
/// 不一致で抜けると、その時間差から署名を 1 バイトずつ当てられてしまう。
fn verify_signature(secret: &[u8], body: &[u8], signature: &[u8]) -> bool {
    // HMAC の鍵は任意長を受け付けるので、この unwrap は落ちない
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(body);

    mac.verify_slice(signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GitHub のドキュメントに載っている検証用の値。
    ///
    /// <https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries>
    const SECRET: &[u8] = b"It's a Secret to Everybody";
    const BODY: &[u8] = b"Hello, World!";
    const SIGNATURE: &str = "757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";

    fn sig(hex_str: &str) -> Vec<u8> {
        hex::decode(hex_str).expect("hex が壊れている")
    }

    /// 正しい署名を受け入れること。
    ///
    /// ここが壊れると全部の webhook が弾かれて通知が止まる。
    #[test]
    fn correct_signature_is_accepted() {
        assert!(verify_signature(SECRET, BODY, &sig(SIGNATURE)));
    }

    /// 署名が違えば弾くこと。
    ///
    /// ここが壊れると誰でも偽の webhook を投げられる。
    #[test]
    fn wrong_signature_is_rejected() {
        // 末尾 1 バイトだけ変える
        let mut bad = sig(SIGNATURE);
        *bad.last_mut().unwrap() ^= 0x01;
        assert!(!verify_signature(SECRET, BODY, &bad));

        // 先頭 1 バイトだけ変える (定数時間比較なので位置に関係なく弾く)
        let mut bad = sig(SIGNATURE);
        bad[0] ^= 0x01;
        assert!(!verify_signature(SECRET, BODY, &bad));
    }

    /// 本文が変わっていれば弾くこと。
    #[test]
    fn tampered_body_is_rejected() {
        assert!(!verify_signature(SECRET, b"Hello, World?", &sig(SIGNATURE)));
    }

    /// 鍵が違えば弾くこと。
    #[test]
    fn wrong_secret_is_rejected() {
        assert!(!verify_signature(b"wrong secret", BODY, &sig(SIGNATURE)));
    }

    /// 長さが違う署名を弾くこと。
    ///
    /// 短い署名を「一致する分だけ」で通してしまうと、1 バイトの署名で
    /// 通過できてしまう。
    #[test]
    fn wrong_length_signature_is_rejected() {
        let full = sig(SIGNATURE);

        assert!(!verify_signature(SECRET, BODY, &[]), "空の署名を通している");
        assert!(
            !verify_signature(SECRET, BODY, &full[..1]),
            "先頭 1 バイトだけの署名を通している"
        );
        assert!(
            !verify_signature(SECRET, BODY, &full[..full.len() - 1]),
            "1 バイト短い署名を通している"
        );

        let mut longer = full.clone();
        longer.push(0);
        assert!(
            !verify_signature(SECRET, BODY, &longer),
            "1 バイト長い署名を通している"
        );
    }

    /// 空の本文でも検証できること (本文なしのイベントは存在する)。
    #[test]
    fn empty_body_is_verified() {
        let mut mac = HmacSha256::new_from_slice(SECRET).unwrap();
        mac.update(b"");
        let expected = mac.finalize().into_bytes();

        assert!(verify_signature(SECRET, b"", &expected));
        assert!(!verify_signature(SECRET, b"", &sig(SIGNATURE)));
    }
}
