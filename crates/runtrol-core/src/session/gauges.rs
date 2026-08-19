//! The account's latest reported position against its limits, per provider.
//!
//! # Why this exists, and why here
//!
//! A limit report is account state, not conversation: the vocabulary says so where [`RateLimit`] is defined, and
//! one provider even declares its report as connection-wide rather than per-thread. Yet the report only ever
//! travelled on individual session event streams, which nobody holds unless a conversation is open on screen. A
//! surface asking "where does each account stand" had nowhere to ask.
//!
//! The session supervisor is the one place every event already passes with its provider known, so the latest
//! report is remembered here as it goes by. Remembered, not interpreted: the fields are the driver's own
//! structured reading, copied as they are.
//!
//! # What is deliberately not kept
//!
//! The provider's verbatim payload. It rides the event stream for whoever is watching, but retaining it here
//! would pin conversation-adjacent bytes in supervisor memory for the lifetime of the process. The gauge keeps
//! only the numbers and the flag, which is also the entire reason it is safe to hand to a surface whose scope is
//! provider structure rather than session output.
//!
//! Bounded by construction: one entry per provider this process has ever run, each a few dozen bytes.

use std::collections::BTreeMap;

use runtrol_provider::{ProviderId, RateLimit, WallMs, Window};

/// One provider's most recent limit report, and when it arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderGauge {
    /// Whose account.
    pub provider: ProviderId,
    /// A limit is blocking right now, by the provider's own word.
    pub reached: bool,
    /// The shorter window, when the provider reports one.
    pub primary: Option<Window>,
    /// The longer window, when the provider reports one.
    pub secondary: Option<Window>,
    /// When the report arrived, which is how a surface says how stale it is.
    pub at: WallMs,
}

/// The latest limit report seen from each provider.
#[derive(Debug, Default)]
pub struct AccountGauges {
    latest: BTreeMap<ProviderId, ProviderGauge>,
}

impl AccountGauges {
    /// Nothing reported yet. `const` so the supervisor's own constructor can stay `const`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            latest: BTreeMap::new(),
        }
    }

    /// Remember one report. The newest always wins, because the gauge answers "now", never "ever".
    pub fn record(&mut self, provider: ProviderId, limit: &RateLimit, at: WallMs) {
        self.latest.insert(
            provider,
            ProviderGauge {
                provider,
                reached: limit.reached,
                primary: limit.primary,
                secondary: limit.secondary,
                at,
            },
        );
    }

    /// Every provider's latest report, in provider order.
    #[must_use]
    pub fn snapshot(&self) -> Vec<ProviderGauge> {
        self.latest.values().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use runtrol_provider::Opaque;

    use super::*;

    fn provider(name: &str) -> ProviderId {
        ProviderId::parse(name).expect("a valid provider id")
    }

    fn report(reached: bool, percent: Option<u8>) -> RateLimit {
        RateLimit {
            primary: Some(Window {
                used_percent: percent,
                resets_at: Some(WallMs::from_millis(1_787_131_200_000)),
                window_minutes: None,
            }),
            secondary: None,
            reached,
            detail: Opaque::owned(r#"{"verbatim":"payload"}"#.to_owned()),
        }
    }

    #[test]
    fn the_newest_report_replaces_the_older_one() {
        // The gauge answers "where does the account stand now". Keeping history would answer a question nobody
        // asked while growing without bound.
        let mut gauges = AccountGauges::default();
        gauges.record(
            provider("claude"),
            &report(false, None),
            WallMs::from_millis(1),
        );
        gauges.record(
            provider("claude"),
            &report(true, Some(97)),
            WallMs::from_millis(2),
        );
        let snapshot = gauges.snapshot();
        assert_eq!(snapshot.len(), 1);
        let gauge = snapshot.first().expect("one provider");
        assert!(gauge.reached);
        assert_eq!(
            gauge.primary.and_then(|window| window.used_percent),
            Some(97)
        );
        assert_eq!(gauge.at, WallMs::from_millis(2));
    }

    #[test]
    fn each_provider_keeps_its_own_gauge() {
        let mut gauges = AccountGauges::default();
        gauges.record(
            provider("claude"),
            &report(false, None),
            WallMs::from_millis(1),
        );
        gauges.record(
            provider("codex"),
            &report(true, Some(87)),
            WallMs::from_millis(1),
        );
        let snapshot = gauges.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert!(
            snapshot
                .iter()
                .any(|gauge| gauge.provider == provider("claude") && !gauge.reached)
        );
        assert!(
            snapshot
                .iter()
                .any(|gauge| gauge.provider == provider("codex") && gauge.reached)
        );
    }

    #[test]
    fn nothing_of_the_verbatim_payload_survives_into_the_gauge() {
        // The payload is conversation-adjacent bytes and the gauge is handed to surfaces whose scope is provider
        // structure. Asserted on the rendering of the whole snapshot so a field added later is covered the day it
        // is written.
        let mut gauges = AccountGauges::default();
        gauges.record(
            provider("claude"),
            &report(false, Some(10)),
            WallMs::from_millis(1),
        );
        let rendered = format!("{:?}", gauges.snapshot());
        assert!(!rendered.contains("verbatim"));
        assert!(!rendered.contains("payload"));
    }
}
