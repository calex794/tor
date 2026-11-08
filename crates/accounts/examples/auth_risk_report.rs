//! Restricted operator command. No public route or automatic account action.
use sqlx::postgres::PgPoolOptions;
use std::{process::ExitCode, time::Duration};
use tor_accounts::repositories::PgAccounts;

#[tokio::main]
async fn main() -> ExitCode {
    match report().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => {
            eprintln!(
                "Отчёт недоступен: проверьте подключение и права чтения. Это не отсутствие угроз."
            );
            ExitCode::FAILURE
        }
    }
}

async fn report() -> Result<(), ()> {
    let url = std::env::var("TOR_AUTH_REPORT_DATABASE_URL").map_err(|_| ())?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&url)
        .await
        .map_err(|_| ())?;
    let signals = PgAccounts::from_pool(pool)
        .auth_risk_signals()
        .await
        .map_err(|_| ())?;
    println!("Диагностика за 15 минут; не основание для автоматической блокировки.");
    println!("тип\tпопытки\tцели\tисточники\tошибки\tуспехи");
    for signal in signals {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            signal.category,
            signal.attempts,
            signal.distinct_targets,
            signal.distinct_sources,
            signal.bad_passwords + signal.unknown_accounts,
            signal.successes,
        );
    }
    Ok(())
}
