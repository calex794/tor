pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[cfg(test)]
mod tests {
    use super::MIGRATOR;
    #[test]
    fn pins_nine_donor_migrations_and_one_cover_extension() {
        assert!(MIGRATOR.migrations.len() >= 10);
        assert_eq!(MIGRATOR.migrations[0].version, 202610120001);
        assert_eq!(MIGRATOR.migrations[8].version, 202610130003);
        assert_eq!(MIGRATOR.migrations[9].version, 202610260001);
        assert!(MIGRATOR
            .migrations
            .windows(2)
            .all(|versions| versions[0].version < versions[1].version));
    }
}
