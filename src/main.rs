use std::pin::Pin;
use std::sync::Arc;

use structopt::StructOpt;

use serde::Deserialize;

use regex::RegexBuilder;

use actix_web::error::ErrorBadRequest;
use actix_web::{web, App, Error, FromRequest, HttpRequest, HttpResponse, HttpServer, Result};

use futures::future::{Future, FutureExt};
use futures::stream::{StreamExt, TryStreamExt};

use tracing::{debug, error, info, warn};

use crypto_hashes::sha2::Sha256;
use hmac::{Hmac, Mac};

type HmacSha256 = Hmac<Sha256>;

mod github;
mod message;
mod slack;

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
    //review_state: Option<String>,
}

#[derive(Debug)]
struct Data {
    json: web::Json<github::Payload>,
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

        use actix_web::error::PayloadError;
        use actix_web::web::Bytes;
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

            let webhook_secret = &opt.webhook_secret.as_bytes();
            let mut mac = HmacSha256::new_from_slice(webhook_secret).unwrap();
            mac.update(&p);
            let result = mac.finalize();

            // validate signature
            if !compare_slice(&sig256, &result.into_bytes()) {
                error!("signature mismatch");
                if !opt.debug {
                    return Err(ErrorBadRequest("signature mismatch!"));
                }
            }

            let b = Bytes::from(p);
            let st = futures::stream::once(async { Ok::<_, PayloadError>(b) });
            let mut p = actix_web::dev::Payload::Stream {
                payload: st.boxed_local(),
            };

            let json = web::Json::<github::Payload>::from_request(&req, &mut p).await?;

            Ok(Data { json }) // validate success
        }
        .boxed_local()
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let opt = Opt::from_args();

    let _guard = sentry::init((
        opt.sentry_dsn.clone(),
        sentry::ClientOptions {
            release: sentry::release_name!(),
            ..Default::default()
        },
    ));
    std::env::set_var("RUST_BACKTRACE", "1");

    let port = opt.hubhook_port;

    let level = if opt.debug {
        tracing::Level::DEBUG
    } else {
        tracing::Level::WARN
    };
    tracing_subscriber::fmt().with_max_level(level).init();

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

    HttpServer::new(move || {
        App::new()
            .wrap(sentry_actix::Sentry::new())
            .app_data(web::Data::new(Arc::new(cfg.clone()))) // memo: https://github.com/actix/actix-web/issues/1454#issuecomment-867897725
            .app_data(web::Data::new(Arc::new(opt.clone())))
            .service(web::resource("/webhook").route(web::post().to(webhook)))
            .service(web::resource("/healthcheck").route(web::get().to(|| HttpResponse::Ok())))
    })
    .bind(format!("0.0.0.0:{}", port))?
    .run()
    .await
}

async fn webhook(
    opt: web::Data<Arc<Opt>>,
    cfg: web::Data<Arc<Config>>,
    data: Option<Data>,
) -> Result<HttpResponse> {
    let payload = if let Some(data) = data {
        data.json
    } else {
        return Ok(HttpResponse::BadRequest().body("bad request"));
    };

    let payload = payload.into_inner();

    //post_test(&opt, &payload).await;

    // match rule
    let matches = payload.match_rules(&cfg.rule);

    for (channel, m) in matches {
        let msg: Result<slack::Message, _> = (&payload).try_into();
        if let Ok(msg) = msg {
            msg.post_message(&opt.slack_token, &channel, Some(&m.display_name))
                .await;
        } else {
            error!(
                "GitHub payload -> slack::Message failed. link = {}",
                &payload.url()
            );
            //error!("payload: {:#?}", &payload);
        }
    }

    Ok(HttpResponse::Ok().body("webhook"))
}

impl Rule {
    fn check_match(&self, payload: &github::Payload) -> bool {
        let include_query_result = Rule::match_results(&self.query, payload).iter().all(|&r| r);

        if let Some(exclude_query) = &self.exclude_query {
            let exclude_query_result = Rule::match_results(exclude_query, payload).iter().any(|&r| r);
            include_query_result && !exclude_query_result
        } else {
            include_query_result
        }
    }

    fn match_results(query: &Query, payload: &github::Payload) -> Vec<bool> {
        let r_repo = Rule::match_query(query.repo.as_ref(), &payload.repo().full_name);

        let topics = &payload.repo().topics;
        let topics = topics.iter().collect();
        let r_topic = Rule::match_query_vec(query.topic.as_ref(), topics);

        let r_sender = Rule::match_query(query.user.as_ref(), &payload.sender().login);
        let r_title = Rule::match_query(query.title.as_ref(), payload.title());
        let r_body = Rule::match_query(query.body.as_ref(), payload.body());

        let labels = payload.labels().iter().collect();
        let r_labels = Rule::match_query_vec(query.label.as_ref(), labels);

        vec![r_repo, r_topic, r_sender, r_title, r_body, r_labels]
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

    fn match_query_impl(query: &str, payload: &str) -> bool {
        if query.is_empty() {
            warn!("query is empty");
            return false;
        }

        let re = RegexBuilder::new(query)
            .case_insensitive(true)
            .build()
            .unwrap();
        if re.is_match(payload) {
            return true;
        }

        false
    }
}

#[allow(dead_code)]
async fn post_test(opt: &Opt, payload: &github::Payload) {
    let msg: slack::Message = payload.try_into().unwrap();
    msg.post_message(&opt.slack_token, "tmp_hubhook", None)
        .await;
}

fn compare_slice(a: &[u8], b: &[u8]) -> bool {
    use std::cmp::Ordering;

    if a.len() != b.len() {
        return false;
    }
    for (ai, bi) in a.iter().zip(b.iter()) {
        match ai.cmp(bi) {
            Ordering::Equal => continue,
            _ => return false,
        }
    }
    true
}
