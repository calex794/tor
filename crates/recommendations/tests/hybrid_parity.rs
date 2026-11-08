//! Execute actual pinned Python and compare every Rust field, including refusals.
use std::path::Path;
use std::process::Command;
use tor_recommendations::services::hybrid_scoring::*;

fn source(value: &str) -> Source {
    [
        Source::Behavioral,
        Source::Content,
        Source::Popularity,
        Source::RelatedBooks,
    ][value.parse::<usize>().unwrap()]
}
fn reason(value: &str) -> Reason {
    [
        Reason::ManuallyRelated,
        Reason::ReadersAlsoRead,
        Reason::SimilarByContent,
        Reason::Trending,
        Reason::Popular,
        Reason::NewRelease,
        Reason::FallbackPopular,
    ][value.parse::<usize>().unwrap()]
}
fn fields(value: &str) -> Vec<&str> {
    value.split(',').collect()
}

#[test]
fn f_07_actual_pinned_python_differential_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("python3")
        .arg(root.join("tools/check_hybrid_parity.py"))
        .output()
        .expect("Python 3 is required for original NEwBOO parity");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut count = 0;
    for line in stdout.lines() {
        let row: Vec<_> = line.split('\t').collect();
        assert_eq!(row.len(), 9, "malformed corpus row");
        let name = row[0];
        let weights = fields(row[1]);
        let candidates: Vec<_> = row[3]
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let f = fields(s);
                Candidate {
                    book_id: f[0].to_owned(),
                    source: source(f[1]),
                    reason: reason(f[2]),
                    raw_score: f[3].parse().unwrap(),
                    normalized_score: if f[4].is_empty() {
                        None
                    } else {
                        Some(f[4].parse().unwrap())
                    },
                    source_book_id: if f[5].is_empty() {
                        None
                    } else {
                        Some(f[5].to_owned())
                    },
                }
            })
            .collect();
        let quality: Vec<_> = row[4]
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let f = fields(s);
                QualitySignal {
                    book_id: f[0].to_owned(),
                    quality_score: f[1].parse().unwrap(),
                    multiplier_per_mille: f[2].parse().unwrap(),
                    trust: [Trust::Normal, Trust::Watch, Trust::Degraded, Trust::Reject]
                        [f[3].parse::<usize>().unwrap()],
                }
            })
            .collect();
        let actual = score_hybrid_candidates(
            &candidates,
            Weights {
                behavioral: weights[0].parse().unwrap(),
                content: weights[1].parse().unwrap(),
                popularity: weights[2].parse().unwrap(),
            },
            row[2].parse().unwrap(),
            &quality,
        );
        let refusal = match actual.rejected_reason {
            None => "accepted",
            Some(Rejection::EmptyCandidates) => "empty_candidates",
            Some(Rejection::InvalidLimit) => "invalid_limit",
            Some(Rejection::InvalidWeights) => "invalid_weights",
            Some(Rejection::MissingNormalizedScores) => "missing_normalized_scores",
        };
        assert_eq!(refusal, row[5], "{name}");
        assert_eq!(
            actual.input_candidates_seen,
            row[6].parse::<usize>().unwrap(),
            "{name}"
        );
        assert_eq!(
            actual.dedicated_candidates_skipped,
            row[7].parse::<usize>().unwrap(),
            "{name}"
        );
        let expected: Vec<_> = row[8]
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let f = fields(s);
                ScoredCandidate {
                    book_id: f[0].to_owned(),
                    final_score: f[1].parse().unwrap(),
                    sources: f[2].split('.').map(source).collect(),
                    reasons: f[3].split('.').map(reason).collect(),
                    best_source: source(f[4]),
                    best_reason: reason(f[5]),
                    rank: f[6].parse().unwrap(),
                }
            })
            .collect();
        assert_eq!(actual.candidates, expected, "{name}");
        count += 1;
    }
    assert_eq!(count, 181, "corpus must not silently lose cases");
}
