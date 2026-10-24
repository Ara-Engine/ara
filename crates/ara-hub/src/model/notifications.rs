//! Toast deadlines use monotonic time; errors and update offers remain until dismissed.
use super::*;
use std::time::{Duration, Instant};

impl Hub {
    /// Advance informational toast deadlines. Idle ticks never invalidate the view.
    pub fn tick(&mut self, now: Instant) -> bool {
        let before = self.notices.len();
        self.notices.retain(|notice| {
            notice.is_error()
                || self.notice_times.get(&notice.id).is_none_or(|created| {
                    let lifetime = if notice.undo.is_some() { 20 } else { 8 };
                    now.saturating_duration_since(*created) < Duration::from_secs(lifetime)
                })
        });
        self.notice_times
            .retain(|id, _| self.notices.iter().any(|notice| notice.id == *id));
        before != self.notices.len()
    }

    /// The next toast expiration, for the native event-loop wait deadline.
    pub fn notice_deadline(&self) -> Option<Instant> {
        self.notices
            .iter()
            .filter(|notice| !notice.is_error())
            .filter_map(|notice| {
                self.notice_times.get(&notice.id).map(|created| {
                    *created + Duration::from_secs(if notice.undo.is_some() { 20 } else { 8 })
                })
            })
            .min()
    }

    /// Whether this instance of an inline notification is still visible.
    pub fn banner_visible(&self, key: &str) -> bool {
        !self.dismissed_banners.contains(key)
    }
}
