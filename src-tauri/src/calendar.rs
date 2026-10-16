//! Shared UTC calendar buckets, read on demand independently of rolling bars.
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct CalendarEntry {
    pub day: String,
    pub model: String,
    pub display_name: String,
    pub tool: String,
    pub member: String,
    pub member_id: String,
    pub account_key: String,
    pub account_label: String,
    pub tokens: i64,
    /// None means unavailable or intentionally unshared pricing, never free usage.
    pub cost: Option<f64>,
    pub cost_complete: bool,
    pub cost_estimated: bool,
}

#[derive(Serialize)]
pub struct CalendarReport {
    pub year: i32,
    pub entries: Vec<CalendarEntry>,
}

pub fn year_bounds(year: i32) -> Result<(String, String), String> {
    use chrono::Datelike;
    let today = chrono::Utc::now().date_naive();
    if !(2000..=today.year()).contains(&year) {
        return Err("Calendar year is outside available history.".into());
    }
    Ok((
        format!("{year}-01-01"),
        format!("{year}-12-31").min(today.to_string()),
    ))
}

pub fn fetch_local(year: i32) -> Result<CalendarReport, String> {
    let (start, end) = year_bounds(year)?;
    let mut entries = Vec::new();
    for (tool, rows) in [
        ("claude-code", crate::localstats::collect_rows(&start, &end)),
        ("codex", crate::codexstats::collect_rows(&start, &end)),
    ] {
        // Fold project/account buckets in the UI; retain dimensions for filters.
        entries.extend(rows.into_iter().map(|r| CalendarEntry {
            day: r.day,
            display_name: crate::localstats::display_name(&r.model),
            model: r.model,
            tool: tool.into(),
            member: String::new(),
            member_id: String::new(),
            account_key: r.account_key,
            account_label: r.account_label,
            tokens: r.tokens,
            cost: (r.tokens == 0 || r.cost > 0.0).then_some(r.cost),
            cost_complete: (r.tokens == 0 || r.cost > 0.0) && r.unattributed == 0,
            cost_estimated: false,
        }));
    }
    entries.extend(crate::openclawstats::collect_calendar(&start, &end));
    Ok(CalendarReport { year, entries })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_validate_and_do_not_overlap_years() {
        assert_eq!(
            year_bounds(2024).unwrap(),
            ("2024-01-01".into(), "2024-12-31".into())
        );
        assert!(year_bounds(1999).is_err());
        assert!(year_bounds(i32::MAX).is_err());
    }
}
