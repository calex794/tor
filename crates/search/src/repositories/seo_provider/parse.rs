//! Strict aggregate decoding. Complementary search queries are deliberately ignored.
use crate::domain::seo_analytics::*;
use serde_json::Value;
use std::collections::BTreeMap;
type E = SeoAnalyticsError;
pub(super) fn number(value: &Value) -> Result<f64, E> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or(E::InvalidResponse)
}
pub(super) fn count(value: &Value) -> Result<u64, E> {
    let n = number(value)?;
    if n.fract() != 0.0 || n > MAX_COUNT as f64 {
        return Err(E::InvalidResponse);
    }
    Ok(n as u64)
}
pub(super) fn google_metrics(row: &Value) -> Result<Metrics, E> {
    let result = Metrics {
        clicks: count(&row["clicks"])?,
        impressions: count(&row["impressions"])?,
        position: row
            .get("position")
            .filter(|v| !v.is_null())
            .map(number)
            .transpose()?,
    };
    if !result.valid() {
        return Err(E::InvalidResponse);
    }
    Ok(result)
}
pub(super) fn google_rows(value: &Value) -> Result<&[Value], E> {
    if !value.is_object() {
        return Err(E::InvalidResponse);
    }
    match value.get("rows") {
        None => Ok(&[]),
        Some(Value::Array(rows)) => Ok(rows),
        _ => Err(E::InvalidResponse),
    }
}
pub(super) fn insert(
    pages: &mut BTreeMap<String, Metrics>,
    origin: &str,
    url: &str,
    metrics: Metrics,
) -> Result<bool, E> {
    let Some(path) = canonical_path(origin, url) else {
        return Ok(false);
    };
    if let Some(previous) = pages.get(&path) {
        if *previous != metrics {
            return Err(E::InvalidResponse);
        }
    } else {
        pages.insert(path, metrics);
    }
    Ok(true)
}
pub(super) fn yandex_metrics(row: &Value, day: &str) -> Result<Option<Metrics>, E> {
    let stats = row["statistics"].as_array().ok_or(E::InvalidResponse)?;
    let mut values = BTreeMap::new();
    for item in stats {
        if item["date"].as_str() != Some(day) {
            continue;
        }
        let key = item["field"].as_str().ok_or(E::InvalidResponse)?;
        if !matches!(key, "CLICKS" | "IMPRESSIONS" | "POSITION") || item["value"].is_null() {
            continue;
        }
        let value = number(&item["value"])?;
        if values.insert(key, value).is_some_and(|old| old != value) {
            return Err(E::InvalidResponse);
        }
    }
    let (Some(clicks), Some(impressions)) = (values.get("CLICKS"), values.get("IMPRESSIONS"))
    else {
        return Ok(None);
    };
    let result = Metrics {
        clicks: count(&Value::from(*clicks))?,
        impressions: count(&Value::from(*impressions))?,
        position: values.get("POSITION").copied(),
    };
    if !result.valid() {
        return Err(E::InvalidResponse);
    }
    Ok(Some(result))
}
pub(super) fn yandex_totals(value: &Value, day: &str) -> Result<Option<Metrics>, E> {
    let obj = value["indicators"].as_object().ok_or(E::InvalidResponse)?;
    let mut values = BTreeMap::new();
    for key in ["TOTAL_CLICKS", "TOTAL_SHOWS", "AVG_SHOW_POSITION"] {
        let Some(rows) = obj.get(key) else { continue };
        for item in rows.as_array().ok_or(E::InvalidResponse)? {
            let date = item["date"].as_str().ok_or(E::InvalidResponse)?;
            if date.get(..10) != Some(day) || item["value"].is_null() {
                continue;
            }
            let value = number(&item["value"])?;
            if values.insert(key, value).is_some_and(|old| old != value) {
                return Err(E::InvalidResponse);
            }
        }
    }
    let (Some(clicks), Some(impressions)) = (values.get("TOTAL_CLICKS"), values.get("TOTAL_SHOWS"))
    else {
        return Ok(None);
    };
    let result = Metrics {
        clicks: count(&Value::from(*clicks))?,
        impressions: count(&Value::from(*impressions))?,
        position: values.get("AVG_SHOW_POSITION").copied(),
    };
    if !result.valid() {
        return Err(E::InvalidResponse);
    }
    Ok(Some(result))
}
