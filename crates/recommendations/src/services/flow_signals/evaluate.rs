use super::{Evaluation, Evidence, Reason, Rule, SignalError, Status};

pub fn evaluate(rule: Rule, evidence: Evidence) -> Result<Evaluation, SignalError> {
    let e = evidence.validate()?;
    let result = if !e.enabled {
        Evaluation::new(Status::Disabled, Reason::SourceDisabled)
    } else if !e.available {
        unknown(Reason::SourceUnavailable)
    } else {
        match rule {
            Rule::HtmlQuality
            | Rule::GoogleQuality
            | Rule::YandexQuality
            | Rule::ExposureQuality => quality(rule, e),
            Rule::UnderexposedBooks | Rule::UnderexposedGenres => underexposed(e),
            Rule::PublicPageLoss => public_pages(e),
            _ => change(rule, e),
        }
    };
    Ok(result)
}
fn unknown(reason: Reason) -> Evaluation {
    Evaluation::new(Status::Unknown, reason)
}
fn active(reason: Reason) -> Evaluation {
    Evaluation::new(Status::Active, reason)
}
fn healthy() -> Evaluation {
    Evaluation::new(Status::Healthy, Reason::WithinThreshold)
}
fn quality(rule: Rule, e: Evidence) -> Evaluation {
    if e.source_failed {
        active(Reason::SourceFailure)
    } else if e.known_discards > 0 {
        active(Reason::DiscardedObservations)
    } else if e.expected_units == 0 || (e.current_units == 0 && e.previous_units == 0) {
        unknown(Reason::MissingMeasurement)
    } else if !e.comparable() {
        active(Reason::PartialCoverage)
    } else if e.limited_units > 0 || e.omitted_rows > 0 {
        active(Reason::LimitedPages)
    } else if rule == Rule::ExposureQuality
        && (e.current_opportunities < 20
            || e.previous_opportunities < 20
            || e.current_ack_per_mille.is_none()
            || e.previous_ack_per_mille.is_none())
    {
        unknown(Reason::ObserverCoverage)
    } else if rule == Rule::ExposureQuality
        && (e.current_ack_per_mille.is_some_and(|n| n < 800)
            || e.previous_ack_per_mille.is_some_and(|n| n < 800))
    {
        active(Reason::ObserverCoverage)
    } else {
        healthy()
    }
}
fn change(rule: Rule, e: Evidence) -> Evaluation {
    if e.source_failed {
        return unknown(Reason::SourceFailure);
    }
    if e.known_discards > 0 {
        return unknown(Reason::DiscardedObservations);
    }
    if !e.comparable() {
        return unknown(Reason::PartialCoverage);
    }
    if rule == Rule::ExposureChange
        && (e.current_sample < 20
            || e.previous_sample < 20
            || e.current_ack_per_mille.is_none_or(|n| n < 800)
            || e.previous_ack_per_mille.is_none_or(|n| n < 800))
    {
        return unknown(Reason::ObserverCoverage);
    }
    let (Some(current), Some(previous)) = (e.current, e.previous) else {
        return unknown(Reason::MissingMeasurement);
    };
    let minimum = match rule {
        Rule::ReadingStarts | Rule::ReadingCompletions => 50,
        _ => 100,
    };
    if previous < minimum {
        return unknown(Reason::SmallBaseline);
    }
    if i128::from(current) * 100 <= i128::from(previous) * 70 {
        active(Reason::RecordedDrop)
    } else {
        healthy()
    }
}
fn underexposed(e: Evidence) -> Evaluation {
    if !e.current_complete() {
        return unknown(if e.source_failed {
            Reason::SourceFailure
        } else {
            Reason::PartialCoverage
        });
    }
    match e.current {
        Some(n) if n > 0 => active(Reason::LowExposure),
        Some(0) if e.unknown_entities == 0 => healthy(),
        _ => unknown(Reason::MissingMeasurement),
    }
}
fn public_pages(e: Evidence) -> Evaluation {
    if e.source_failed {
        return unknown(Reason::SourceFailure);
    }
    match e.current {
        Some(n) if n > 0 => active(Reason::PublicPageLoss),
        Some(0) if e.current_complete() && e.unknown_entities == 0 => healthy(),
        _ => unknown(Reason::MissingMeasurement),
    }
}
