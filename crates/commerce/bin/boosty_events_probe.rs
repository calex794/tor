//! Operator-only Boosty event observer. No database connection is opened.
use serde::Serialize;
use std::{collections::HashSet, env};
use tor_commerce::{
    repositories::boosty_events::BoostyEventFeed,
    services::boosty_events::{parse_events, valid_blog, ObserveError, ObservedEvent},
};

struct Options {
    blog: String,
    limit: usize,
    max_pages: usize,
}

impl Options {
    fn parse() -> Result<Self, ObserveError> {
        let args: Vec<String> = env::args().skip(1).collect();
        let mut blog = None;
        let mut limit = 50;
        let mut max_pages = 3;
        let mut seen = HashSet::new();
        let (chunks, remainder) = args.as_chunks::<2>();
        for pair in chunks {
            if !seen.insert(&pair[0]) {
                return Err(ObserveError::InvalidConfig);
            }
            match pair[0].as_str() {
                "--blog" => blog = Some(pair[1].clone()),
                "--limit" => limit = pair[1].parse().map_err(|_| ObserveError::InvalidConfig)?,
                "--max-pages" => {
                    max_pages = pair[1].parse().map_err(|_| ObserveError::InvalidConfig)?
                }
                _ => return Err(ObserveError::InvalidConfig),
            }
        }
        if !remainder.is_empty() || !(1..=100).contains(&limit) || !(1..=5).contains(&max_pages) {
            return Err(ObserveError::InvalidConfig);
        }
        let blog: String = blog.ok_or(ObserveError::InvalidConfig)?;
        if !valid_blog(&blog) {
            return Err(ObserveError::InvalidConfig);
        }
        Ok(Self {
            blog,
            limit,
            max_pages,
        })
    }
}

#[derive(Serialize)]
struct Report {
    observer_mode: &'static str,
    money_posted: bool,
    pages_read: usize,
    rows_examined: usize,
    complete: bool,
    events: Vec<ObservedEvent>,
    warning: &'static str,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Boosty observer failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse()?;
    let token = env::var("TOR_BOOSTY_PROBE_TOKEN").map_err(|_| ObserveError::InvalidConfig)?;
    let feed = BoostyEventFeed::new(&token)?;
    let mut cursor: Option<String> = None;
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    let mut rows_examined = 0;
    let mut pages_read = 0;
    let mut complete = false;
    for _ in 0..options.max_pages {
        let bytes = feed
            .fetch(&options.blog, options.limit, cursor.as_deref())
            .await?;
        let page = parse_events(&bytes)?;
        pages_read += 1;
        rows_examined += page.rows_examined;
        events.extend(page.events);
        if page.is_last || page.rows_examined == 0 {
            complete = true;
            break;
        }
        let next = page.next_offset.ok_or(ObserveError::Pagination)?;
        if !seen.insert(next.clone()) {
            return Err(ObserveError::Pagination.into());
        }
        cursor = Some(next);
    }
    let report = Report {
        observer_mode: "read-only; operator-initiated",
        money_posted: false,
        pages_read,
        rows_examined,
        complete,
        events,
        warning: "Event IDs and amounts are NOT verified settled payments. Never use diagnostic fingerprints as credit IDs.",
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
