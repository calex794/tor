//! Operator-only bounded DROP inventory, using runtime audit components.
#![forbid(unsafe_code)]

#[cfg(feature = "s3-free")]
mod audit {
    use aws_sdk_s3::{
        config::{
            retry::RetryConfig, timeout::TimeoutConfig, BehaviorVersion, Credentials, Region,
        },
        Client,
    };
    use serde_json::json;
    use std::{
        env,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use tor_books::repositories::{
        free_inventory_cycles::{
            CompletedInventoryReport, InventoryDirection, InventoryNamespace,
            PgFreeInventoryCheckpoints,
        },
        PgCatalog,
    };
    use tor_web::free_pack_worker::inventory_operator::{
        OperatorObservedScanner, OperatorScanStep,
    };

    const USAGE: &str = "usage: drop_inventory_audit scan observed|expected single|pack <1-32>\n       drop_inventory_audit report observed|expected single|pack";

    fn setting(name: &'static str) -> Result<String, &'static str> {
        env::var(name)
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or(name)
    }

    fn s3_client() -> Result<Client, &'static str> {
        let endpoint = setting("TOR_DROP_AUDIT_S3_ENDPOINT")?;
        let parsed = reqwest::Url::parse(&endpoint).map_err(|_| "invalid S3 endpoint")?;
        let loopback = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1"));
        if !(parsed.scheme() == "https" || parsed.scheme() == "http" && loopback)
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
        {
            return Err("invalid S3 endpoint");
        }
        Ok(Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new(setting("TOR_DROP_AUDIT_S3_REGION")?))
                .endpoint_url(endpoint)
                .credentials_provider(Credentials::new(
                    setting("TOR_DROP_AUDIT_S3_ACCESS_KEY")?,
                    setting("TOR_DROP_AUDIT_S3_SECRET_KEY")?,
                    None,
                    None,
                    "drop-audit-operator",
                ))
                .force_path_style(true)
                .retry_config(RetryConfig::standard().with_max_attempts(1))
                .timeout_config(
                    TimeoutConfig::builder()
                        .operation_timeout(Duration::from_secs(12))
                        .build(),
                )
                .build(),
        ))
    }

    fn counters(report: &CompletedInventoryReport) -> [i64; 6] {
        [
            report.inspected_count,
            report.matched_count,
            report.protected_count,
            report.unknown_count,
            report.mismatch_count,
            report.unverifiable_count,
        ]
    }

    fn report_line(report: &CompletedInventoryReport) {
        println!(
            "{}",
            json!({
                "cycle": report.id, "pages": report.page_index, "completed_unix": report.completed_unix,
                "inspected": report.inspected_count, "matched": report.matched_count,
                "protected": report.protected_count, "unknown": report.unknown_count,
                "mismatch": report.mismatch_count, "unverifiable": report.unverifiable_count
            })
        );
    }

    #[tokio::main]
    pub async fn run() -> Result<(), &'static str> {
        let args: Vec<_> = env::args().skip(1).collect();
        let (command, direction, namespace, pages) = match args.as_slice() {
            [command, direction, namespace] if command == "report" => {
                (command, direction, namespace, 0)
            }
            [command, direction, namespace, pages] if command == "scan" => {
                let pages = pages.parse::<u32>().map_err(|_| USAGE)?;
                if !(1..=32).contains(&pages) {
                    return Err(USAGE);
                }
                (command, direction, namespace, pages)
            }
            _ => return Err(USAGE),
        };
        let direction = match direction.as_str() {
            "observed" => InventoryDirection::ObservedToPg,
            "expected" => InventoryDirection::ExpectedToS3,
            _ => return Err(USAGE),
        };
        let namespace = match namespace.as_str() {
            "single" => InventoryNamespace::Single,
            "pack" => InventoryNamespace::Pack,
            _ => return Err(USAGE),
        };
        let bucket = setting("TOR_DROP_AUDIT_BUCKET")?;
        let pg = PgCatalog::connect(&setting("TOR_DROP_AUDIT_DATABASE_URL")?)
            .await
            .map_err(|_| "PostgreSQL connection failed")?;
        if command == "report" {
            let reports = PgFreeInventoryCheckpoints::new(pg.pool().clone())
                .completed_reports(&bucket, direction, namespace)
                .await
                .map_err(|_| "completed audit reports unavailable")?;
            for report in &reports {
                report_line(report);
            }
            let comparison = match reports.as_slice() {
                [a, b] if counters(a) == counters(b) => "same_aggregate_counts",
                [_, _] => "different_aggregate_counts",
                _ => "two_completed_cycles_required",
            };
            println!(
                "{}",
                json!({"comparison": comparison, "gc_authorized": false})
            );
            return Ok(());
        }

        let scanner = OperatorObservedScanner::new(&pg, s3_client()?, bucket)
            .map_err(|_| "invalid audit configuration")?;
        for _ in 0..pages {
            let step = match direction {
                InventoryDirection::ObservedToPg => {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|_| "system clock unavailable")?
                        .as_secs() as i64;
                    scanner.run_one(namespace, now).await
                }
                InventoryDirection::ExpectedToS3 => scanner.run_expected_one(namespace).await,
            }
            .map_err(|_| "audit page could not be confirmed")?;
            let completed = matches!(step, OperatorScanStep::Completed { .. });
            match step {
                OperatorScanStep::Busy => {
                    println!("{}", json!({"status": "busy_or_retry_limit"}));
                    break;
                }
                OperatorScanStep::Advanced {
                    cycle_id,
                    page_index,
                    counters,
                }
                | OperatorScanStep::Completed {
                    cycle_id,
                    page_index,
                    counters,
                } => println!(
                    "{}",
                    json!({
                        "cycle": cycle_id, "page": page_index, "completed": completed,
                        "inspected": counters.inspected, "registered": counters.registered,
                        "protected_pending": counters.protected_pending, "unknown": counters.unknown,
                        "verified": counters.verified, "mismatch": counters.mismatch,
                        "unverifiable": counters.unverifiable, "sealed": counters.sealed
                    })
                ),
            }
            if completed {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(feature = "s3-free")]
fn main() {
    if let Err(error) = audit::run() {
        eprintln!("DROP_INVENTORY_REFUSED: {error}");
        std::process::exit(2);
    }
}

#[cfg(not(feature = "s3-free"))]
fn main() {
    eprintln!("DROP_INVENTORY_REFUSED: build with s3-free");
    std::process::exit(2);
}
