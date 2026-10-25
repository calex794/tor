pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[cfg(test)]
mod tests {
    use super::MIGRATOR;
    #[test]
    fn pins_nine_donor_migrations() {
        assert_eq!(MIGRATOR.migrations.len(), 9);
        assert_eq!(MIGRATOR.migrations[0].version, 202610120001);
        assert_eq!(MIGRATOR.migrations[8].version, 202610130003);
    }
}
