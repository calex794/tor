//! Read-only serving: bounded generation lookup, unchanged hybrid math, final live B gate.
use super::*;
use crate::domain::feedback::{rollout_allowed, Feed, FeedItem, ALGORITHM, MAX_ITEMS};
use crate::services::{hybrid_scoring::{score_hybrid_candidates, Candidate, Reason, Source}, scenario::{weights, ScenarioFacts}};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};

fn source(value:&str) -> Result<Source,FeedbackError> {
    match value {
        "behavioral"=>Ok(Source::Behavioral), "content"=>Ok(Source::Content),
        "popularity"=>Ok(Source::Popularity), "related_books"=>Ok(Source::RelatedBooks),
        _=>Err(FeedbackError::Unavailable),
    }
}
fn reason(value:&str) -> Result<Reason,FeedbackError> {
    match value {
        "readers_also_read"=>Ok(Reason::ReadersAlsoRead),"similar_by_content"=>Ok(Reason::SimilarByContent),
        "popular"=>Ok(Reason::Popular),"trending"=>Ok(Reason::Trending),"new_release"=>Ok(Reason::NewRelease),
        "fallback_popular"=>Ok(Reason::FallbackPopular),"manually_related"=>Ok(Reason::ManuallyRelated),
        _=>Err(FeedbackError::Unavailable),
    }
}
fn source_name(value:Source) -> &'static str {
    match value {Source::Behavioral=>"behavioral",Source::Content=>"content",Source::Popularity=>"popularity",Source::RelatedBooks=>"related_books"}
}
fn reason_name(value:Reason) -> &'static str {
    match value {
        Reason::ReadersAlsoRead=>"readers_also_read",Reason::SimilarByContent=>"similar_by_content",
        Reason::Popular=>"popular",Reason::Trending=>"trending",Reason::NewRelease=>"new_release",
        Reason::FallbackPopular=>"fallback_popular",Reason::ManuallyRelated=>"manually_related",
    }
}

impl PgRecommendations {
    pub async fn feed(&self, actor:Option<i64>, seed:Option<i64>, limit:usize) -> Result<Feed,FeedbackError> {
        if !(1..=MAX_ITEMS).contains(&limit) || actor.is_some_and(|a|a<=0) || seed.is_some_and(|a|a<=0) {
            return Err(FeedbackError::Invalid);
        }
        if let Some(seed)=seed {
            let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rec_public_books WHERE id=$1)")
                .bind(seed).fetch_one(&self.pool).await?;
            if !allowed {return Err(FeedbackError::NotFound);}
        }
        let subject:Option<String>=sqlx::query_scalar("SELECT subject_id::text FROM rec_subjects WHERE user_id=$1 AND enabled")
            .bind(actor).fetch_optional(&self.pool).await?;
        let recent:Vec<i64>=if let Some(subject)=subject.as_ref() {
            sqlx::query_scalar("SELECT s.book_id FROM rec_reader_signals s JOIN rec_public_books b ON b.id=s.book_id WHERE s.subject_id=$1::uuid AND s.occurred_at>now()-interval '30 days' ORDER BY s.occurred_at DESC,s.book_id LIMIT 5")
                .bind(subject).fetch_all(&self.pool).await?
        } else {Vec::new()};
        let (generation,_,_,percent,hold)=self.runtime_status().await?;
        let eligible=!hold && generation.is_some() && match subject.as_deref() {
            Some(subject)=>rollout_allowed(subject,percent), None=>percent==100,
        };
        if !eligible {return self.fallback(subject.as_deref(),seed,limit).await;}
        let generation=generation.expect("eligible generation");
        let mut seeds=recent.clone();
        if let Some(seed)=seed {seeds.insert(0,seed);}
        seeds.sort_unstable(); seeds.dedup(); seeds.truncate(5); seeds.push(0);
        let rows=sqlx::query("SELECT seed_id,book_id,source,reason,score FROM rec_candidate_edges WHERE generation_id=$1::uuid AND seed_id=ANY($2::bigint[]) ORDER BY seed_id,score DESC,book_id::text LIMIT 500")
            .bind(&generation).bind(&seeds).fetch_all(&self.pool).await?;
        let mut candidates=Vec::new();
        let mut dedicated=BTreeSet::new();
        for row in rows {
            let id:i64=row.try_get("book_id")?;
            if Some(id)==seed || recent.contains(&id) {continue;}
            let candidate_source=source(&row.try_get::<String,_>("source")?)?;
            let candidate_reason=reason(&row.try_get::<String,_>("reason")?)?;
            let score=i64::from(row.try_get::<i16,_>("score")?);
            if candidate_source==Source::RelatedBooks {
                dedicated.insert(id.to_string());
            } else {
                candidates.push(Candidate {
                    book_id:id.to_string(),source:candidate_source,reason:candidate_reason,
                    raw_score:score,normalized_score:Some(score),
                    source_book_id:Some(row.try_get::<i64,_>("seed_id")?.to_string()),
                });
            }
        }
        let behavioral=candidates.iter().filter(|c|c.source==Source::Behavioral).count();
        let facts=ScenarioFacts { recent_books:recent.len(),recent_signals:recent.len(),behavioral_candidates:behavioral,source_edges:behavioral,..ScenarioFacts::default() };
        let mut ordered:Vec<(i64,String,String)>=dedicated.into_iter().take(10)
            .map(|id| Ok((id.parse::<i64>().map_err(|_|FeedbackError::Unavailable)?,"related_books".to_owned(),"manually_related".to_owned())))
            .collect::<Result<_,FeedbackError>>()?;
        if !candidates.is_empty() {
            // REC-13 observation does not silently alter legacy quality weights.
            let decision=score_hybrid_candidates(&candidates,weights(&facts),50,&[]);
            if decision.rejected_reason.is_some() {return Err(FeedbackError::Unavailable);}
            for item in decision.candidates {
                let id=item.book_id.parse::<i64>().map_err(|_|FeedbackError::Unavailable)?;
                if !ordered.iter().any(|c|c.0==id) {
                    ordered.push((id,source_name(item.best_source).to_owned(),reason_name(item.best_reason).to_owned()));
                }
            }
        }
        ordered.truncate(60);
        if ordered.is_empty() {return self.fallback(subject.as_deref(),seed,limit).await;}
        let ids:Vec<i64>=ordered.iter().map(|c|c.0).collect();
        // One final statement checks current B visibility and current explicit preference.
        let rows=sqlx::query("SELECT b.id,b.title,COALESCE(f.revision,0) AS revision FROM rec_public_books b LEFT JOIN rec_feedback_state f ON f.subject_id=$2::uuid AND f.book_id=b.id WHERE b.id=ANY($1::bigint[]) AND NOT(COALESCE(f.hidden,false) AND f.expires_at>now())")
            .bind(&ids).bind(subject.as_deref()).fetch_all(&self.pool).await?;
        let mut metadata=BTreeMap::new();
        for row in rows {metadata.insert(row.try_get::<i64,_>("id")?,(row.try_get::<String,_>("title")?,row.try_get::<i64,_>("revision")?));}
        let mut items=Vec::new();
        for (id,source,reason) in ordered {
            if let Some((title,revision))=metadata.remove(&id) {
                items.push(FeedItem { book_id:id.to_string(),title,source,reason,algorithm_version:ALGORITHM,ticket:None,feedback_revision:subject.as_ref().map(|_|revision) });
                if items.len()==limit {break;}
            }
        }
        Ok(Feed {items,generation_id:Some(generation),next_cursor:None,personalized:subject.is_some()})
    }

    async fn fallback(&self, subject:Option<&str>, seed:Option<i64>, limit:usize) -> Result<Feed,FeedbackError> {
        let rows=sqlx::query("SELECT b.id,b.title,COALESCE(f.revision,0) AS revision FROM rec_public_books b LEFT JOIN rec_feedback_state f ON f.subject_id=$1::uuid AND f.book_id=b.id WHERE ($2::bigint IS NULL OR b.id<>$2) AND NOT(COALESCE(f.hidden,false) AND f.expires_at>now()) ORDER BY b.created_at DESC,b.id DESC LIMIT $3")
            .bind(subject).bind(seed).bind(limit as i64).fetch_all(&self.pool).await?;
        let mut items=Vec::new();
        for row in rows {
            items.push(FeedItem {book_id:row.try_get::<i64,_>("id")?.to_string(),title:row.try_get("title")?,source:"popularity".to_owned(),reason:"new_release".to_owned(),algorithm_version:"public_fallback_v1",ticket:None,feedback_revision:subject.map(|_|row.try_get::<i64,_>("revision")).transpose()?});
        }
        Ok(Feed {items,generation_id:None,next_cursor:None,personalized:false})
    }
}
