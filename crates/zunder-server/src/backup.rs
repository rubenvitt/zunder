//! Nightly backup at `ZUNDER_BACKUP_HOUR` (local time).

use chrono::{DateTime, Days, LocalResult, TimeZone};

use crate::app::App;

/// The next full hour `hour:00` (local time of `now`'s zone) strictly after
/// `now`. Skips over a DST gap by taking the first valid instant after it.
pub fn next_run<Tz: TimeZone>(now: &DateTime<Tz>, hour: u8) -> DateTime<Tz> {
    let tz = now.timezone();
    let today = now.date_naive();
    for offset in 0..=2u64 {
        let date = today + Days::new(offset);
        let Some(naive) = date.and_hms_opt(u32::from(hour), 0, 0) else {
            continue;
        };
        let candidate = match tz.from_local_datetime(&naive) {
            LocalResult::Single(t) => Some(t),
            LocalResult::Ambiguous(first, _) => Some(first),
            // DST gap: the wall clock skips this hour; run right after it.
            LocalResult::None => tz
                .from_local_datetime(&(naive + chrono::Duration::hours(1)))
                .earliest(),
        };
        if let Some(t) = candidate.filter(|t| t > now) {
            return t;
        }
    }
    // Unreachable for valid hours; fall back to 24 hours later.
    now.clone() + chrono::Duration::hours(24)
}

/// Runs the backup every night until the app shuts down.
pub async fn run_nightly(app: App) {
    let shutdown = app.shutdown_token();
    let hour = app.config().backup_hour;
    loop {
        let now = chrono::Local::now();
        let next = next_run(&now, hour);
        let wait = (next - now).to_std().unwrap_or_default();
        tracing::info!(next = %next.to_rfc3339(), "next backup scheduled");
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
        if let Err(e) = app.backup_now().await {
            tracing::error!(error = %e, "nightly backup failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Utc};

    #[test]
    fn later_today_or_tomorrow() {
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 1, 30, 0).unwrap();
        assert_eq!(
            next_run(&now, 3),
            Utc.with_ymd_and_hms(2026, 10, 11, 3, 0, 0).unwrap()
        );
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 3, 0, 0).unwrap();
        assert_eq!(
            next_run(&now, 3),
            Utc.with_ymd_and_hms(2026, 10, 12, 3, 0, 0).unwrap()
        );
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let now = tz.with_ymd_and_hms(2026, 12, 31, 23, 59, 0).unwrap();
        assert_eq!(
            next_run(&now, 0),
            tz.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap()
        );
    }
}
