mod admin;
mod budget;
mod cache;
mod cli;
mod config;
mod context;
mod cookies;
mod crawl;
mod doctor;
mod engine;
mod errors;
mod fetch;
mod finish;
mod guard;
mod jinadeep;
mod logging;
mod outline;
mod output;
mod page;
mod providers;
mod reading;
mod registry;
mod relevance;
mod replay;
mod rerank;
mod research;
mod research_tasks;
mod rounds;
mod router;
mod run;
mod stats;
mod synth;
mod tools;
mod types;
#[cfg(feature = "ui")]
mod ui;
mod usage;
mod verify;
mod wait;

use clap::Parser;
use cli::{Cli, Commands, ConfigAction};
use context::AppContext;
use std::io::IsTerminal;
use std::sync::Arc;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn init_tracing(debug: bool) {
    use tracing_subscriber::{fmt, EnvFilter};
    let default = if debug {
        "debug,hyper=warn,hyper_util=warn,reqwest=warn,h2=warn,rustls=warn,tower=warn,chromiumoxide=warn,tungstenite=warn"
    } else {
        "warn,chromiumoxide=off"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal() && !output::no_color_env())
        .with_target(false)
        .without_time()
        .try_init();
}

/// `osearch help run` and `osearch run --help` print the request help with
/// the defaults of the loaded config, which clap's static help cannot.
fn run_help_asked() -> bool {
    let args: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with("--debug") && a != "--json").collect();
    matches!(args.as_slice(), [a, b] if a == "help" && b == "run")
        || matches!(args.as_slice(), [a, b] if a == "run" && (b == "--help" || b == "-h"))
}

fn main() {
    // The keys file sets environment variables, which must happen before
    // any other thread starts.
    if let Ok(c) = config::load_config() {
        config::load_env_file(&c.paths.envfile);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    runtime.block_on(start());
}

async fn start() {
    if run_help_asked() {
        match config::load_config() {
            Ok(c) => config::init(&c),
            Err(e) => eprintln!("(config {}: {e}; showing the built-in defaults)", config::config_path().display()),
        }
        println!("{}", cli::request_help());
        return;
    }
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => e.exit(),
    };
    init_tracing(cli.debug);
    let config = match config::load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: config {}: {e}", config::config_path().display());
            std::process::exit(2);
        }
    };
    config::init(&config);
    guard::set_allow_private(cli.allow_private);

    // Every key this process holds is scrubbed from any error text.
    errors::register_secrets(
        providers::all()
            .iter()
            .map(|s| providers::key_for(&config, s))
            .chain(
                providers::all()
                    .iter()
                    .flat_map(|s| s.env_keys.iter())
                    .filter_map(|k| std::env::var(k).ok()),
            ),
    );
    let app = Arc::new(AppContext::new(config));
    let json = cli.json;
    let code = match run(cli, app).await {
        Ok(code) => code,
        Err(e) => {
            if json {
                output::json::render_error(&e);
            } else {
                eprintln!("Error: {}", errors::redact_secrets(&e.to_string()));
                if let Some(s) = e.suggestion() {
                    eprintln!("  {s}");
                }
            }
            e.exit_code()
        }
    };
    std::process::exit(code);
}

fn print_outcome(o: &research::Outcome, json: bool) {
    if json {
        output::json::render_value(&serde_json::json!({
            "status": "success",
            "run_dir": o.run_dir.to_string_lossy(),
            "summary": o.summary,
        }));
    } else {
        print!("{}", o.summary);
    }
}

/// The questions of the quick form. Every argument that holds a space is a
/// question of its own; arguments with none (`osearch crawl4ai alternatives`)
/// are one question. A mode before a colon ("community: ...") is that
/// question's mode when it is exactly a mode name.
fn quick_questions(words: &[String]) -> Vec<(Option<String>, String)> {
    let texts: Vec<String> = if words.iter().any(|w| w.contains(char::is_whitespace)) {
        words.iter().map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect()
    } else {
        vec![words.join(" ")]
    };
    texts.iter().map(|t| research::split_mode(t)).collect()
}

fn search_request(questions: Vec<(Option<String>, String)>, o: cli::QueryOpts) -> research::Request {
    let topic = questions.iter().map(|(_, q)| q.as_str()).collect::<Vec<_>>().join(" / ");
    research::Request {
        topic,
        parent: o.parent,
        queries: questions
            .into_iter()
            .map(|(mode, q)| {
                research::QueryIn::Spec(research::QuerySpec {
                    q,
                    mode: mode.or_else(|| o.mode.clone()),
                    engines: o.providers.clone(),
                    kind: o.kind.clone(),
                    count: o.count,
                    sites: o.domain.clone(),
                    exclude: o.exclude_domain.clone(),
                    since: o.freshness.clone(),
                    region: o.region.clone(),
                    lang: o.lang.clone(),
                })
            })
            .collect(),
        notes: Vec::new(),
        urls: Vec::new(),
        fetch: o.fetch,
        snowball: o.snowball,
        platforms: o.platforms,
        cost_cap: o.cost_cap,
        no_paid_readers: o.no_paid,
        region: None,
        via: None,
        no_cache: o.no_cache,
        quick: true,
        width: o.width,
        fetch_time: o.fetch_time.or(Some(config::get().research.quicktime)),
        reddit_login: o.reddit_login,
        code_search: o.code_search,
        synth: if o.synth {
            Some(true)
        } else if o.no_synth {
            Some(false)
        } else {
            None
        },
        jina_deepsearch: if o.jina_deepsearch {
            Some(research::JinaAsk::On(true))
        } else if o.no_jina_deepsearch {
            Some(research::JinaAsk::On(false))
        } else {
            None
        },
        subreddits: o.subreddits,
        fresh: o.fresh,
    }
}

async fn run(cli: Cli, app: Arc<AppContext>) -> Result<i32, errors::SearchError> {
    let json = cli.json;
    let command = match cli.command {
        Some(c) => c,
        None if !cli.query_words.is_empty() => {
            let questions = quick_questions(&cli.query_words);
            let o = research::run(app, search_request(questions, cli.quick)).await?;
            print_outcome(&o, json);
            return Ok(0);
        }
        None => {
            use clap::CommandFactory;
            Cli::command().print_long_help().ok();
            println!();
            return Ok(0);
        }
    };
    match command {
        Commands::Search(a) => {
            if a.query.trim().is_empty() {
                return Err(errors::SearchError::InvalidInput {
                    message: "query is empty".into(),
                });
            }
            let o = research::run(app, search_request(vec![(None, a.query)], a.o)).await?;
            print_outcome(&o, json);
            Ok(0)
        }
        Commands::Run { file } => {
            let text = std::fs::read_to_string(&file)?;
            let bad = |e: serde_json::Error| errors::SearchError::InvalidInput { message: format!("{file}: {e}") };
            let mut req: research::Request = serde_json::from_str(&text).map_err(bad)?;
            req.notes = research::unknown_fields(&serde_json::from_str(&text).map_err(bad)?);
            if req.queries.is_empty() && req.urls.is_empty() {
                return Err(errors::SearchError::InvalidInput {
                    message: "the request has no queries and no urls".into(),
                });
            }
            let o = research::run(app, req).await?;
            print_outcome(&o, json);
            Ok(0)
        }
        Commands::Verify { emails } => {
            let results = verify::verify_emails(&emails).await?;
            if json {
                output::json::render_value(&serde_json::json!({ "status": "success", "results": results }));
            } else {
                verify::render_table(&results);
            }
            Ok(0)
        }
        Commands::Fetch { finish: Some(run), .. } => finish::finish(app, &run).await,
        Commands::Fetch { wait: true, run, .. } => {
            print!("{}", finish::wait(run.as_deref().unwrap_or("last")).await?);
            Ok(0)
        }
        Commands::Fetch {
            urls,
            parent,
            platforms,
            no_paid,
            via,
            fresh,
            ..
        } => {
            let req = research::Request {
                fresh,
                topic: format!("fetch {}", urls.first().cloned().unwrap_or_default()),
                parent,
                urls,
                fetch: Some(0),
                snowball: Some(0),
                platforms,
                no_paid_readers: no_paid,
                via,
                ..research::Request::default()
            };
            let o = research::run(app, req).await?;
            print_outcome(&o, json);
            Ok(0)
        }
        Commands::Crawl {
            url,
            topic,
            max_pages,
            include,
            exclude,
            resume,
            any_path,
        } => {
            let o = crawl::crawl(
                app,
                crawl::CrawlArgs {
                    url,
                    topic,
                    max_pages: max_pages.clamp(1, 500),
                    include,
                    exclude,
                    resume,
                    any_path,
                },
            )
            .await?;
            print_outcome(&o, json);
            Ok(0)
        }
        Commands::Read {
            page,
            run,
            full,
            find,
        } => {
            print!("{}", reading::read(&page, run.as_deref(), full, find.as_deref())?);
            Ok(0)
        }
        Commands::Grep { pattern, run, max, .. } => {
            print!("{}", reading::grep(&pattern, run.as_deref(), max)?);
            Ok(0)
        }
        Commands::Runs { compress: true, .. } => {
            let days = config::get().research.compressafter;
            if days == 0 {
                println!("Compression is off: set research.compressafter to a number of days first (`osearch config set research.compressafter 30`).");
                return Ok(2);
            }
            let c = run::compress_old(days);
            println!(
                "Compressed {} files in {} runs older than {days} days: {:.1} MB to {:.1} MB. Pages, summaries and syntheses are unchanged; osearch reads the compressed files as before.",
                c.files,
                c.runs,
                c.before as f64 / 1e6,
                c.after as f64 / 1e6
            );
            Ok(0)
        }
        Commands::Runs { find, limit, .. } => {
            print!("{}", reading::runs(find.as_deref(), limit));
            Ok(0)
        }
        Commands::Login(a) => admin::login(a).await,
        Commands::Stats { prune: Some(n), .. } => {
            eprintln!("Pruned {} log files older than {n} days", stats::prune_logs(n));
            Ok(0)
        }
        Commands::Stats { days, .. } => {
            let report = stats::compute(days);
            if json {
                output::json::render_value(&serde_json::json!({ "status": "success", "stats": report }));
            } else {
                stats::render_human(&report);
            }
            Ok(0)
        }
        Commands::Usage => {
            let report = usage::report(app).await;
            if json {
                output::json::render_value(&serde_json::json!({ "status": "success", "usage": report }));
            } else {
                usage::render_human(&report);
            }
            Ok(0)
        }
        Commands::Doctor { probe } => {
            let ok = doctor::run(app, probe, json).await;
            Ok(if ok { 0 } else { 1 })
        }
        Commands::Update { check } => admin::update(app, check).await,
        Commands::Providers => {
            admin::providers(&app);
            Ok(0)
        }
        Commands::Config { action } => {
            match action {
                ConfigAction::Show => config::config_show(&app.config),
                ConfigAction::Set { key, value } => {
                    config::config_set(&key, &value)?;
                    eprintln!("Set {key}");
                }
                ConfigAction::Path => println!("{}", config::config_path().display()),
            }
            Ok(0)
        }
        Commands::Synth { run, wait, all } => synth::command(app, run, wait, all).await,
        Commands::Wait { run, jina_deepsearch } => wait::command(run, jina_deepsearch).await,
        Commands::Replay { run, top } => replay::command(&app, run, top).await,
        #[cfg(feature = "ui")]
        Commands::Ui { port, no_open } => ui::serve(app, port, !no_open).await,
        Commands::JinaDeepsearch { job } => jinadeep::job(&job).await,
        Commands::Research {
            question,
            engines,
            depth,
            approved,
            parent,
            collect,
            wait,
        } => {
            let o = research_tasks::research(
                app,
                research_tasks::ResearchArgs {
                    question: question.join(" "),
                    engines,
                    depth,
                    approved,
                    parent,
                    collect,
                    wait,
                },
            )
            .await?;
            print_outcome(&o, json);
            Ok(0)
        }
        Commands::CacheClear => {
            eprintln!("Removed {} cached entries", cache::clear());
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::quick_questions;

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quoted_arguments_are_separate_questions_and_bare_words_are_one() {
        assert_eq!(quick_questions(&words(&["crawl4ai", "alternatives"])).len(), 1);
        assert_eq!(quick_questions(&words(&["kopia", "vs", "restic"]))[0].1, "kopia vs restic");
        let q = quick_questions(&words(&["kopia vs restic", "community: borg backup problems"]));
        assert_eq!(q.len(), 2);
        assert_eq!(q[1], (Some("community".to_string()), "borg backup problems".to_string()));
    }
}
