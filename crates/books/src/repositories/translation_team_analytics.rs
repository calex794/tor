//! S029 Team analytics use committed reader, purchase and thanks facts only.
//! No guessed unique visitors and no Book entitlement bypass.
#![forbid(unsafe_code)]
use crate::services::translation_teams::TeamError;
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeamPeriod {
    Week,
    Month,
    Quarter,
}
impl TeamPeriod {
    pub fn parse(value: Option<&str>) -> Result<Self, TeamError> {
        match value.unwrap_or("30d") {
            "7d" => Ok(Self::Week),
            "30d" => Ok(Self::Month),
            "90d" => Ok(Self::Quarter),
            _ => Err(TeamError::Invalid),
        }
    }
    pub fn days(self) -> i32 {
        match self {
            Self::Week => 7,
            Self::Month => 30,
            Self::Quarter => 90,
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Self::Week => "7d",
            Self::Month => "30d",
            Self::Quarter => "90d",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TeamNumbers {
    pub opens: i64,
    pub anonymous_opens: i64,
    pub first_chapter_opens: i64,
    pub new_registered_readers: i64,
    pub purchases: i64,
    /// Gross purchase sums in the currency's own minor units; never sum unlike currencies.
    pub gross_by_currency_minor: BTreeMap<String, i64>,
    pub thanks: i64,
}
impl TeamNumbers {
    fn add(&mut self, other: &Self) {
        self.opens = self.opens.saturating_add(other.opens);
        self.anonymous_opens = self.anonymous_opens.saturating_add(other.anonymous_opens);
        self.first_chapter_opens = self
            .first_chapter_opens
            .saturating_add(other.first_chapter_opens);
        self.new_registered_readers = self
            .new_registered_readers
            .saturating_add(other.new_registered_readers);
        self.purchases = self.purchases.saturating_add(other.purchases);
        for (currency, amount) in &other.gross_by_currency_minor {
            let aggregate = self.gross_by_currency_minor.entry(currency.clone()).or_default();
            *aggregate = aggregate.saturating_add(*amount);
        }
        self.thanks = self.thanks.saturating_add(other.thanks);
    }
}

fn parse_currency_totals(value: Option<String>) -> Result<BTreeMap<String, i64>, TeamError> {
    let mut currencies = BTreeMap::new();
    if let Some(serialized) = value {
        for item in serialized.split('|') {
            let (currency, raw) = item.split_once(':').ok_or(TeamError::Unavailable)?;
            if currency.len() != 3 || !currency.bytes().all(|letter| letter.is_ascii_uppercase()) {
                return Err(TeamError::Unavailable);
            }
            let amount = raw.parse::<i64>().map_err(|_| TeamError::Unavailable)?;
            if amount < 0 || currencies.insert(currency.to_owned(), amount).is_some() {
                return Err(TeamError::Unavailable);
            }
        }
    }
    Ok(currencies)
}

#[derive(Clone, Debug)]
pub struct TeamAnalyticsBook {
    pub id: i64,
    pub title: String,
    pub numbers: TeamNumbers,
}
#[derive(Clone, Debug)]
pub struct TeamAnalyticsReport {
    pub team_name: String,
    pub period: TeamPeriod,
    pub date_from: String,
    pub date_to: String,
    pub rows: Vec<TeamAnalyticsBook>,
    pub totals: TeamNumbers,
}
#[derive(Clone)]
pub struct PgTeamAnalytics {
    pool: PgPool,
}

impl PgTeamAnalytics {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// HTTP page and CSV call this *same* query and filter contract.
    /// All book titles and metrics are scoped to active member+Book association
    /// in SQL, not solely by a potentially stale pre-query authorization.
    pub async fn report(
        &self,
        actor: i64,
        team_id: i64,
        selected_book_id: Option<i64>,
        period: TeamPeriod,
    ) -> Result<TeamAnalyticsReport, TeamError> {
        if actor <= 0 || team_id <= 0 || selected_book_id.is_some_and(|id| id <= 0) {
            return Err(TeamError::Invalid);
        }
        let team_name: String = sqlx::query_scalar(
            "SELECT t.name FROM translation_teams t
             JOIN users u ON u.id=$1 AND u.account_state='active'
             JOIN users creator ON creator.id=t.owner_user_id AND creator.account_state='active'
             WHERE t.id=$2 AND (t.owner_user_id=$1 OR EXISTS (
                 SELECT 1 FROM translation_team_members m WHERE m.team_id=t.id AND m.user_id=$1
             ))",
        )
        .bind(actor)
        .bind(team_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| TeamError::Unavailable)?
        .ok_or(TeamError::Missing)?;

        let rows = sqlx::query(
            "WITH boundaries AS (
                SELECT (now() AT TIME ZONE 'UTC')::DATE AS end_day,
                       (now() AT TIME ZONE 'UTC')::DATE - ($4::INT - 1) AS start_day
             ), scope AS (
                SELECT b.id,b.title FROM translation_teams t
                JOIN translation_team_books tb ON tb.team_id=t.id AND tb.is_active
                JOIN books b ON b.id=tb.book_id AND b.owner_user_id=tb.added_by_user_id
                JOIN users actor ON actor.id=$2 AND actor.account_state='active'
                JOIN users creator ON creator.id=t.owner_user_id AND creator.account_state='active'
                WHERE t.id=$1 AND NOT b.is_deleted AND NOT b.admin_hold
                  AND ($3::BIGINT IS NULL OR b.id=$3)
                  AND (t.owner_user_id=$2 OR EXISTS (
                    SELECT 1 FROM translation_team_members me
                    WHERE me.team_id=t.id AND me.user_id=$2
                  ))
                  AND (b.owner_user_id=t.owner_user_id OR EXISTS (
                    SELECT 1 FROM translation_team_members owner_member
                    JOIN users owner_user ON owner_user.id=owner_member.user_id
                        AND owner_user.account_state='active'
                    WHERE owner_member.team_id=t.id AND owner_member.user_id=b.owner_user_id
                  ))
                ORDER BY tb.created_at DESC,b.id DESC LIMIT 100
             ), opened AS (
                SELECT d.book_id,
                       SUM(d.opens)::BIGINT AS opens,
                       SUM(d.anonymous_opens)::BIGINT AS anonymous,
                       SUM(d.first_authenticated_opens)::BIGINT AS firsts
                FROM book_chapter_open_daily d
                JOIN scope s ON s.id=d.book_id CROSS JOIN boundaries w
                WHERE d.day BETWEEN w.start_day AND w.end_day GROUP BY d.book_id
             ), newcomers AS (
                SELECT r.book_id,COUNT(*)::BIGINT AS newcomers
                FROM book_open_readers r JOIN scope s ON s.id=r.book_id
                CROSS JOIN boundaries w
                WHERE (r.first_opened_at AT TIME ZONE 'UTC')::DATE
                  BETWEEN w.start_day AND w.end_day GROUP BY r.book_id
             ), purchase_currencies AS (
                SELECT c.book_id,p.currency,COUNT(*)::BIGINT AS purchases,
                       SUM(p.amount_minor)::BIGINT AS gross_minor
                FROM chapter_purchase_facts p JOIN chapters c ON c.id=p.chapter_id
                JOIN scope s ON s.id=c.book_id CROSS JOIN boundaries w
                WHERE (p.acquired_at AT TIME ZONE 'UTC')::DATE
                  BETWEEN w.start_day AND w.end_day GROUP BY c.book_id,p.currency
             ), purchases AS (
                SELECT book_id,SUM(purchases)::BIGINT AS purchases,
                       STRING_AGG(currency::TEXT || ':' || gross_minor::TEXT,
                           '|' ORDER BY currency) AS gross_by_currency
                FROM purchase_currencies GROUP BY book_id
             ), thanks AS (
                SELECT x.book_id,COUNT(*)::BIGINT AS thanks
                FROM chapter_thanks x JOIN scope s ON s.id=x.book_id
                CROSS JOIN boundaries w
                WHERE (x.first_thanked_at AT TIME ZONE 'UTC')::DATE
                  BETWEEN w.start_day AND w.end_day GROUP BY x.book_id
             )
             SELECT TO_CHAR(w.start_day,'YYYY-MM-DD') AS from_day,
                    TO_CHAR(w.end_day,'YYYY-MM-DD') AS to_day,
                    s.id,s.title,
                    COALESCE(o.opens,0)::BIGINT AS opens,
                    COALESCE(o.anonymous,0)::BIGINT AS anonymous,
                    COALESCE(o.firsts,0)::BIGINT AS firsts,
                    COALESCE(n.newcomers,0)::BIGINT AS newcomers,
                    COALESCE(p.purchases,0)::BIGINT AS purchases,
                    p.gross_by_currency AS gross_by_currency,
                    COALESCE(th.thanks,0)::BIGINT AS thanks
             FROM boundaries w LEFT JOIN scope s ON TRUE
             LEFT JOIN opened o ON o.book_id=s.id
             LEFT JOIN newcomers n ON n.book_id=s.id
             LEFT JOIN purchases p ON p.book_id=s.id
             LEFT JOIN thanks th ON th.book_id=s.id
             ORDER BY s.id",
        )
        .bind(team_id)
        .bind(actor)
        .bind(selected_book_id)
        .bind(period.days())
        .fetch_all(&self.pool)
        .await
        .map_err(|_| TeamError::Unavailable)?;
        let mut books = Vec::new();
        let mut totals = TeamNumbers::default();
        let mut date_from = String::new();
        let mut date_to = String::new();
        for row in rows {
            date_from = row
                .try_get("from_day")
                .map_err(|_| TeamError::Unavailable)?;
            date_to = row.try_get("to_day").map_err(|_| TeamError::Unavailable)?;
            let book_id: Option<i64> = row.try_get("id").map_err(|_| TeamError::Unavailable)?;
            let Some(id) = book_id else {
                continue;
            };
            let numbers = TeamNumbers {
                opens: row.try_get("opens").map_err(|_| TeamError::Unavailable)?,
                anonymous_opens: row
                    .try_get("anonymous")
                    .map_err(|_| TeamError::Unavailable)?,
                first_chapter_opens: row.try_get("firsts").map_err(|_| TeamError::Unavailable)?,
                new_registered_readers: row
                    .try_get("newcomers")
                    .map_err(|_| TeamError::Unavailable)?,
                purchases: row
                    .try_get("purchases")
                    .map_err(|_| TeamError::Unavailable)?,
                gross_by_currency_minor: parse_currency_totals(row.try_get("gross_by_currency").map_err(|_| TeamError::Unavailable)?)?,
                thanks: row.try_get("thanks").map_err(|_| TeamError::Unavailable)?,
            };
            totals.add(&numbers);
            books.push(TeamAnalyticsBook {
                id,
                title: row.try_get("title").map_err(|_| TeamError::Unavailable)?,
                numbers,
            });
        }
        if selected_book_id.is_some() && books.is_empty() {
            return Err(TeamError::Missing);
        }
        Ok(TeamAnalyticsReport {
            team_name,
            period,
            date_from,
            date_to,
            rows: books,
            totals,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_currency_totals, TeamNumbers, TeamPeriod};
    #[test]
    fn s029_periods_reject_unknown_and_use_fixed_windows() {
        assert_eq!(TeamPeriod::parse(None).unwrap().days(), 30);
        assert_eq!(TeamPeriod::parse(Some("7d")).unwrap().days(), 7);
        assert_eq!(TeamPeriod::parse(Some("90d")).unwrap().days(), 90);
        assert!(TeamPeriod::parse(Some("365d")).is_err());
    }

    #[test]
    fn s029_purchase_sums_never_combine_different_currencies() {
        let a = parse_currency_totals(Some("RUB:150|USD:245".to_owned())).unwrap();
        let b = parse_currency_totals(Some("RUB:300|EUR:75".to_owned())).unwrap();
        let mut total = TeamNumbers {
            gross_by_currency_minor: a,
            ..TeamNumbers::default()
        };
        total.add(&TeamNumbers {
            gross_by_currency_minor: b,
            ..TeamNumbers::default()
        });
        assert_eq!(total.gross_by_currency_minor.get("RUB"), Some(&450));
        assert_eq!(total.gross_by_currency_minor.get("USD"), Some(&245));
        assert_eq!(total.gross_by_currency_minor.get("EUR"), Some(&75));
        assert!(parse_currency_totals(Some("RUB:1|RUB:2".to_owned())).is_err());
        assert!(parse_currency_totals(Some("BAD:abc".to_owned())).is_err());
    }
}
