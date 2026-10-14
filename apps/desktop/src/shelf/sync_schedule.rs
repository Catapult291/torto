use std::time::{Duration, Instant};

use crate::sync::SyncMode;

const FULL_CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);
const MAX_READING_DELAY: Duration = Duration::from_secs(15);
const RETRY_DELAY: Duration = Duration::from_secs(30);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SyncSchedule {
    pub derived_token: Option<String>,
    last_full: Instant,
    pending_since: Option<Instant>,
    retry_after: Instant,
    queued: Option<SyncMode>,
    failures: u32,
    rate_limited_until: Option<Instant>,
}

impl Default for SyncSchedule {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            derived_token: None,
            last_full: now,
            pending_since: None,
            retry_after: now,
            queued: None,
            failures: 0,
            rate_limited_until: None,
        }
    }
}

impl SyncSchedule {
    pub fn rate_limit_deadline(&self, now: Instant) -> Option<Instant> {
        self.rate_limited_until.filter(|deadline| *deadline > now)
    }

    pub fn started(&mut self, mode: SyncMode) {
        if matches!(mode, SyncMode::Full { .. })
            || self.queued == Some(SyncMode::Reading)
            || mode == SyncMode::Derived && self.queued == Some(SyncMode::Derived)
        {
            self.queued = None;
        }
        self.pending_since = None;
    }

    pub fn request(&mut self, mode: SyncMode) {
        self.queued = Some(match (self.queued, mode) {
            (
                Some(SyncMode::Full {
                    force_statistics: previous,
                }),
                SyncMode::Full {
                    force_statistics: next,
                },
            ) => SyncMode::Full {
                force_statistics: previous || next,
            },
            (Some(full @ SyncMode::Full { .. }), _) => full,
            (_, full @ SyncMode::Full { .. }) => full,
            (Some(SyncMode::Derived), _) | (_, SyncMode::Derived) => SyncMode::Derived,
            (_, mode) => mode,
        });
    }

    pub fn next(
        &mut self,
        now: Instant,
        now_ms: u64,
        latest_change_ms: Option<u64>,
    ) -> Option<SyncMode> {
        if now < self.retry_after {
            return None;
        }
        // A stream of local derived changes must not postpone remote discovery.
        if now.duration_since(self.last_full) >= FULL_CHECK_INTERVAL
            && self.queued == Some(SyncMode::Derived)
        {
            self.queued = None;
            return Some(SyncMode::Full {
                force_statistics: false,
            });
        }
        if let Some(mode) = self.queued.take() {
            return Some(mode);
        }
        if let Some(latest) = latest_change_ms {
            let since = *self.pending_since.get_or_insert(now);
            if now_ms.saturating_sub(latest) >= 2_000
                || now.duration_since(since) >= MAX_READING_DELAY
            {
                self.pending_since = None;
                return Some(SyncMode::Reading);
            }
        } else {
            self.pending_since = None;
        }
        (now.duration_since(self.last_full) >= FULL_CHECK_INTERVAL).then_some(SyncMode::Full {
            force_statistics: false,
        })
    }

    pub fn completed(&mut self, mode: SyncMode, success: bool, now: Instant) {
        self.completed_with_limit(mode, success, now, None);
    }

    pub fn completed_with_limit(
        &mut self,
        mode: SyncMode,
        success: bool,
        now: Instant,
        rate_limit: Option<Duration>,
    ) {
        if success {
            if matches!(mode, SyncMode::Full { .. }) {
                self.last_full = now;
            }
            self.retry_after = now;
            self.failures = 0;
            self.rate_limited_until = None;
        } else {
            self.request(mode);
            self.failures = self.failures.saturating_add(1);
            let base = if rate_limit.is_some() {
                Duration::from_secs(5 * 60)
            } else {
                RETRY_DELAY
            };
            let delay = base
                .saturating_mul(1_u32 << (self.failures - 1).min(6))
                .min(MAX_RETRY_DELAY)
                .max(rate_limit.unwrap_or_default());
            self.retry_after = now + delay;
            if rate_limit.is_some() {
                self.rate_limited_until = Some(self.retry_after);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_work_coalesces_but_cannot_starve_full_discovery() {
        let mut schedule = SyncSchedule::default();
        let now = schedule.last_full;
        schedule.request(SyncMode::Derived);
        schedule.request(SyncMode::Reading);
        assert_eq!(schedule.next(now, 0, None), Some(SyncMode::Derived));
        schedule.completed(SyncMode::Derived, true, now);
        schedule.request(SyncMode::Derived);
        assert_eq!(
            schedule.next(now + FULL_CHECK_INTERVAL, 0, None),
            Some(SyncMode::Full {
                force_statistics: false
            })
        );
        schedule.request(SyncMode::Full {
            force_statistics: true,
        });
        schedule.request(SyncMode::Derived);
        assert_eq!(
            schedule.next(now + FULL_CHECK_INTERVAL, 0, None),
            Some(SyncMode::Full {
                force_statistics: true
            })
        );
    }

    #[test]
    fn reading_sync_debounces_but_continuous_turns_cannot_starve_it() {
        let mut schedule = SyncSchedule::default();
        let now = schedule.last_full;
        assert_eq!(schedule.next(now, 1_000, Some(1_000)), None);
        assert_eq!(
            schedule.next(now + Duration::from_secs(1), 2_000, Some(1_800)),
            None
        );
        assert_eq!(
            schedule.next(now + Duration::from_secs(3), 4_000, Some(1_800)),
            Some(SyncMode::Reading)
        );
        assert_eq!(
            schedule.next(now + Duration::from_secs(4), 5_000, Some(5_000)),
            None
        );
        assert_eq!(
            schedule.next(now + Duration::from_secs(19), 20_000, Some(20_000)),
            Some(SyncMode::Reading)
        );
    }

    #[test]
    fn returning_to_library_flushes_immediately_and_failed_work_retries() {
        let mut schedule = SyncSchedule::default();
        let now = schedule.last_full;
        schedule.request(SyncMode::Reading);
        assert_eq!(
            schedule.next(now, 1_000, Some(1_000)),
            Some(SyncMode::Reading)
        );
        schedule.completed(SyncMode::Reading, false, now);
        assert_eq!(
            schedule.next(now + Duration::from_secs(29), 30_000, Some(1_000)),
            None
        );
        assert_eq!(
            schedule.next(now + Duration::from_secs(30), 31_000, Some(1_000)),
            Some(SyncMode::Reading)
        );
    }

    #[test]
    fn consecutive_failures_back_off_and_success_resets_the_delay() {
        let mut schedule = SyncSchedule::default();
        let mut now = schedule.last_full;
        for delay in [30, 60, 120, 240, 480, 960, 1800, 1800] {
            schedule.completed(SyncMode::Reading, false, now);
            assert_eq!(
                schedule.next(now + Duration::from_secs(delay - 1), 0, None),
                None
            );
            now += Duration::from_secs(delay);
            assert_eq!(schedule.next(now, 0, None), Some(SyncMode::Reading));
        }
        schedule.completed(SyncMode::Reading, true, now);
        schedule.completed(SyncMode::Reading, false, now);
        assert_eq!(
            schedule.next(now + Duration::from_secs(30), 0, None),
            Some(SyncMode::Reading)
        );
    }

    #[test]
    fn rate_limit_keeps_full_work_queued_and_respects_long_server_deadlines() {
        let mut schedule = SyncSchedule::default();
        let now = schedule.last_full;
        schedule.completed_with_limit(
            SyncMode::Reading,
            false,
            now,
            Some(Duration::from_secs(7200)),
        );
        let full = SyncMode::Full {
            force_statistics: false,
        };
        schedule.request(full);
        assert!(schedule.rate_limit_deadline(now).is_some());
        assert_eq!(
            schedule.next(now + Duration::from_secs(7199), 0, None),
            None
        );
        assert_eq!(
            schedule.next(now + Duration::from_secs(7200), 0, None),
            Some(full)
        );
        schedule.completed(full, true, now + Duration::from_secs(7200));
        assert_eq!(
            schedule.rate_limit_deadline(now + Duration::from_secs(7200)),
            None
        );
    }
}
