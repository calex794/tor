use super::*;

fn fixture(days: i32) -> (ObservedAnchor, Vec<ObservedHour>) {
    let until = 1_800_000_000;
    let hours = observed_hours(days).unwrap() as i64;
    (
        ObservedAnchor {
            until_epoch: until,
            measured_at_utc: "2027-01-15T08:00:00Z".into(),
            until_utc: "end".into(),
            current_from_utc: "middle".into(),
            previous_from_utc: "start".into(),
            persisted_discards: 0,
        },
        (0..2 * hours)
            .map(|index| ObservedHour {
                epoch: until - (2 * hours - index) * 3_600,
                local_label: "fixture".into(),
                offset_minutes: 0,
                weekday: index as usize / 24 % 7,
                local_hour: index as usize % 24,
                views: [None; 4],
                sources: [None; 4],
            })
            .collect(),
    )
}

#[test]
fn all_periods_keep_every_unknown_hour_and_exact_period_boundaries() {
    for days in [1, 7, 30, 90] {
        let (anchor, rows) = fixture(days);
        let report = observed_report(days, "UTC", anchor, rows).unwrap();
        assert_eq!(report.current.hours.len(), days as usize * 24);
        assert_eq!(report.current.from_epoch, report.previous.until_epoch);
        assert_eq!(report.current.saved_total, None);
        assert_eq!(report.previous.recorded_hours, 0);
        assert_eq!(report.current.weekday_hour, [[None; 24]; 7]);
    }
    for days in [-1, 0, 8, 91, i32::MAX] {
        assert_eq!(observed_hours(days), Err(FlowHourlyError::Invalid));
    }
    for zone in ["", "UTC ", "utc", "<script>", "Europe/Unknown"] {
        assert_eq!(validate_zone(zone), Err(FlowHourlyError::Invalid));
    }
}

#[test]
fn measured_zero_is_not_unknown_and_overlapping_dst_clock_hours_both_count() {
    let (anchor, mut rows) = fixture(1);
    rows[24].views = [Some(0), None, None, None];
    rows[24].sources = [None, None, None, Some(0)];
    rows[25].views = [Some(2), None, None, Some(3)];
    rows[25].sources = [Some(5), None, None, None];
    rows[26].views = [None, Some(7), None, None];
    rows[26].sources = [None, Some(7), None, None];
    // Fall-back repeated wall-clock hour: distinct UTC keys remain separate.
    for index in [25, 26] {
        rows[index].weekday = 6;
        rows[index].local_hour = 2;
    }
    rows[25].offset_minutes = 120;
    rows[26].offset_minutes = 60;
    let report = observed_report(1, "Europe/Amsterdam", anchor, rows).unwrap();
    assert_eq!(report.current.saved_total, Some(12));
    assert_eq!(report.current.surfaces, [Some(2), Some(7), None, Some(3)]);
    assert_eq!(report.current.weekday_hour[6][2], Some(12));
    assert_eq!(report.current.recorded_slots[6][2], 2);
    assert_eq!(report.current.recorded_hours, 3);
    assert_eq!(report.current.hours[0].views[0], Some(0));
}

#[test]
fn malformed_rows_negative_counts_and_overflow_fail_without_partial_report() {
    for error in 0..8 {
        let (mut anchor, mut rows) = fixture(1);
        match error {
            0 => {
                rows.pop();
            }
            1 => rows[1].epoch = rows[0].epoch,
            2 => rows[1].weekday = 7,
            3 => rows[1].local_hour = 24,
            4 => {
                rows[1].views[0] = Some(-1);
                rows[1].sources[0] = Some(-1);
            }
            5 => {
                rows[1].views[0] = Some(3);
                rows[1].sources[0] = Some(4);
            }
            6 => anchor.persisted_discards = -1,
            _ => {
                for index in [1, 2] {
                    rows[index].views[0] = Some(i64::MAX);
                    rows[index].sources[0] = Some(i64::MAX);
                }
            }
        }
        assert_eq!(
            observed_report(1, "UTC", anchor, rows),
            Err(FlowHourlyError::Unavailable)
        );
    }
}
