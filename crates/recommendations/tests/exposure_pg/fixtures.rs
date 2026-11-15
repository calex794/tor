use super::*;
pub struct Fixture {
    pub pool: PgPool,
    pub store: PgExposure,
    pub books: Vec<i64>,
    pub actor: i64,
    pub label: String,
}
impl Fixture {
    pub async fn new(count: usize) -> Self {
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&current_analytics::url("TEST_DATABASE_URL", "tor_app"))
            .await
            .unwrap();
        current_analytics::assert_schema(&pool).await;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let label = format!("obs-{stamp}");
        let actor=sqlx::query_scalar("INSERT INTO users(email_normalized,display_name,password_hash) VALUES($1,'Observation fixture','$argon2id$test') RETURNING id").bind(format!("{label}@tor-ci.example")).fetch_one(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE books IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await
            .unwrap();
        let mut books = vec![];
        for i in 0..count {
            let id=sqlx::query_scalar("INSERT INTO books(id,source_identity,target_language,title,visibility,access_level,genres,tags) SELECT coalesce(max(id),0)+1,$1,'ru',$2,'public','basic',ARRAY[$3,$3],ARRAY['shared','shared'] FROM books RETURNING id").bind(format!("ci:{label}:{i}")).bind(format!("Observe <script> {i}")).bind(&label).fetch_one(&mut *tx).await.unwrap();
            books.push(id);
        }
        tx.commit().await.unwrap();
        Self {
            store: PgExposure::new(pool.clone()),
            pool,
            books,
            actor,
            label,
        }
    }
    pub fn card(&self, n: usize) -> CardObservation {
        CardObservation {
            book_id: self.books[n],
            surface: ExposureSurface::Home,
            editorial: None,
        }
    }
    pub fn filter(&self) -> ExposureFilter {
        ExposureFilter {
            days: 7,
            surface: None,
            after: 0,
            class: None,
            group: Some(("genre".into(), self.label.clone())),
            campaign: None,
            editorial: None,
        }
    }
    pub async fn counts(&self, n: usize) -> (i64, i64, i64, i64, i64) {
        sqlx::query_as("SELECT sum(opportunities)::bigint,sum(rendered)::bigint,sum(views)::bigint,sum(clicks)::bigint,sum(replayed)::bigint FROM flow_observation_hourly WHERE book_id=$1").bind(self.books[n]).fetch_one(&self.pool).await.unwrap()
    }
    pub async fn book_change(&self, assignment: &str) {
        sqlx::query(&format!("UPDATE books SET {assignment} WHERE id=$1"))
            .bind(self.books[0])
            .execute(&self.pool)
            .await
            .unwrap();
    }
    pub async fn cleanup(self) {
        let mut tx = self.pool.begin().await.unwrap();
        sqlx::query("SELECT live_proofs FROM flow_observation_state WHERE singleton FOR UPDATE")
            .execute(&mut *tx)
            .await
            .unwrap();
        let deleted =
            sqlx::query("DELETE FROM flow_observation_proofs WHERE book_id=ANY($1::bigint[])")
                .bind(&self.books)
                .execute(&mut *tx)
                .await
                .unwrap()
                .rows_affected() as i64;
        sqlx::query("UPDATE flow_observation_state SET live_proofs=live_proofs-$1 WHERE singleton")
            .bind(deleted)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("DELETE FROM flow_observation_hourly WHERE book_id=ANY($1::bigint[])")
            .bind(&self.books)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("DELETE FROM books WHERE id=ANY($1::bigint[])")
            .bind(&self.books)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(self.actor)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        self.pool.close().await;
    }
}
pub fn event(proof: &str, kind: EventKind) -> ObservationEvent {
    ObservationEvent {
        proof: proof.into(),
        kind,
        visible_percent: 60,
        continuous_visible_ms: 1000,
        foreground: true,
    }
}
