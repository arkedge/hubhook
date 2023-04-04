use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use actix_web::http::header::HeaderMap;
use structopt::StructOpt;

use serde::{Deserialize, Serialize};

use regex::RegexBuilder;

use actix_web::error::ErrorBadRequest;
use actix_web::{web, App, Error, FromRequest, HttpRequest, HttpResponse, HttpServer, Result};

use futures::future::{Future, FutureExt};
use futures::stream::TryStreamExt;

use tracing::{debug, error, info, warn};

use crypto_hashes::sha2::Sha256;
use hmac::{Hmac, Mac};

use crate::message::{CreatedMessage, IntoMessage};

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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayloadKind {
    IssueComment,
    Issues,
    PullRequest,
}

impl PayloadKind {
    const HEADER_NAME: &str = "x-github-event";
    fn from_header(headers: &HeaderMap) -> Option<Self> {
        serde_json::from_slice(headers.get(Self::HEADER_NAME)?.as_bytes()).ok()
    }
}

#[derive(Debug)]
struct Extracted {
    payload_kind: PayloadKind,
    payload: Vec<u8>,
}

impl FromRequest for Extracted {
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

        let sig256: Vec<u8> = {
            let sig = headers.get("x-hub-signature-256").unwrap();
            let sig = std::str::from_utf8(sig.as_bytes()).unwrap();
            let sig = sig.strip_prefix("sha256=").unwrap();
            hex::decode(sig).unwrap()
        };
        let payload_kind = match PayloadKind::from_header(headers) {
            Some(p) => p,
            None => {
                error!("missing header value {}", PayloadKind::HEADER_NAME);
                return Box::pin(err(ErrorBadRequest("missing header")));
            }
        };

        let (debug, webhook_secret) = {
            let opt = req.app_data::<web::Data<Arc<Opt>>>().unwrap();
            (opt.debug, opt.webhook_secret.clone())
        };
        let pd = payload.take();
        async move {
            let p = pd
                .try_fold(Vec::new(), |mut acc, chunk| async {
                    acc.extend(chunk);
                    Ok(acc)
                })
                .await;
            let payload_bytes: Vec<u8> = p.unwrap();
            let mut mac = HmacSha256::new_from_slice(webhook_secret.as_bytes()).unwrap();
            mac.update(&payload_bytes);

            // validate signature
            if mac.verify_slice(&sig256).is_err() {
                error!("signature mismatch");
                if !debug {
                    return Err(ErrorBadRequest("signature mismatch!"));
                }
            }

            Ok(Extracted {
                payload: payload_bytes,
                payload_kind,
            }) // validate success
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
    })
    .bind(format!("0.0.0.0:{}", port))?
    .run()
    .await
}

async fn webhook(
    opt: web::Data<Arc<Opt>>,
    cfg: web::Data<Arc<Config>>,
    Extracted {
        payload_kind,
        payload,
    }: Extracted,
) -> Result<HttpResponse> {
    let map_err = |e| {
        error!(
            "failed to parse payload {}: {e}",
            serde_json::to_string(&payload_kind).unwrap()
        );
        ErrorBadRequest("failed to parse payload")
    };
    use github::Payload::*;
    let payload = match &payload_kind {
        PayloadKind::IssueComment => {
            IssueComment(serde_json::from_slice(&payload).map_err(map_err)?)
        }
        PayloadKind::Issues => Issues(serde_json::from_slice(&payload).map_err(map_err)?),
        PayloadKind::PullRequest => PullRequest(serde_json::from_slice(&payload).map_err(map_err)?),
    };

    //post_test(&opt, &payload).await;

    // match rule
    let matches = match_rules(&payload, &cfg.rule);

    if !matches.is_empty() {
        use CreatedMessage::*;
        match payload.create_message() {
            Ok(msg) => {
                for (channel, m) in matches {
                    msg.post_message(&opt.slack_token, &channel, Some(&m.display_name))
                        .await;
                }
            }
            SkipThisEvent => (),
        }
    }

    Ok(HttpResponse::Ok().body("webhook"))
}

impl Rule {
    fn check_match(&self, payload: &github::Payload) -> bool {
        let query = &self.query;

        let r_repo = Rule::match_query(query.repo.as_ref(), payload.repo().full_name);

        let topics = &payload.repo().topics;
        let topics = topics.iter().collect();
        let r_topic = Rule::match_query_vec(query.topic.as_ref(), topics);

        let r_sender = Rule::match_query(query.user.as_ref(), payload.sender().login);
        let r_title = Rule::match_query(query.title.as_ref(), payload.title());
        let r_body = Rule::match_query(query.body.as_ref(), payload.body());

        let labels = payload.labels().iter().collect();
        let r_labels = Rule::match_query_vec(query.label.as_ref(), labels);

        vec![r_repo, r_topic, r_sender, r_title, r_body, r_labels]
            .iter()
            .flatten()
            .collect::<Vec<&bool>>()
            .iter()
            .all(|x| **x)
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

pub fn match_rules(payload: &github::Payload, rules: &[Rule]) -> HashMap<String, RuleMatchResult> {
    let mut v = HashMap::<String, RuleMatchResult>::new();

    for r in rules {
        // not match
        if !r.check_match(payload) {
            continue;
        }

        // multiple display_name
        let mut display_name = r.display_name.clone();
        if let Some(res) = v.get(&r.channel) {
            display_name = res.display_name.to_string() + "&" + &display_name;
        }

        let res = RuleMatchResult {
            display_name,
            channel: r.channel.clone(),
        };
        v.insert(r.channel.clone(), res);
    }

    v
}

#[allow(dead_code)]
async fn post_test<'a>(opt: &Opt, payload: &github::Payload<'a>) {
    let msg = payload.create_message();
    msg.as_ok()
        .unwrap()
        .post_message(&opt.slack_token, "tmp_hubhook", None)
        .await;
}
