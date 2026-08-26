//! One reading of this CLI's account limits, shared by the two surfaces that publish them.
//!
//! # Why one module
//!
//! This CLI states its limits twice in the same shape: `account/rateLimits/read` answers on request, and
//! `account/rateLimits/updated` arrives unasked during a turn. Both carry its `RateLimitSnapshot`, and its
//! generated schema defines that type once. Reading it in two places let the two readings drift, and they
//! had: the turn path dropped every reset instant while the request path kept them.
//!
//! # What the snapshot actually contains
//!
//! Measured 2026-08-26 on 0.149.1, signed in on a `pro` plan, from the CLI's own generated JSON Schema and a
//! live `account/rateLimits/read`:
//!
//! ```text
//! rateLimitsByLimitId: {
//!   "codex":           { limitName: null, primary: { usedPercent: 29, windowDurationMins: 10080, resetsAt: … } },
//!   "codex_bengalfox": { limitName: "GPT-5.3-Codex-Spark",
//!                        primary:   { usedPercent: 0, windowDurationMins:   300, resetsAt: … },
//!                        secondary: { usedPercent: 0, windowDurationMins: 10080, resetsAt: … } },
//! }
//! ```
//!
//! So the account has three live windows, not two, and one of them is scoped to a single model the CLI names
//! for itself. A pair of `primary`/`secondary` slots could carry at most one bucket, which is why the whole
//! second bucket used to be invisible.
//!
//! # `resetsAt` is unix seconds
//!
//! Established by reading it, not by guessing: the request answer gives `1788300917`, which is six days from
//! the measurement and matches the seven-day window it belongs to. Milliseconds would put it three weeks
//! after the epoch. The turn notification declares the same `RateLimitWindow` type in the same schema
//! bundle, so it is the same unit there, and the turn path now keeps the instant it used to discard.

use std::collections::BTreeMap;

use runtrol_provider::{Window, account_token};
use serde::Deserialize;

/// The identity a window carries when this CLI names no bucket for it.
///
/// Not the provider's own name: this is the account's default bucket, and calling it after the product
/// would collide the moment the CLI adds a bucket it actually calls that.
const DEFAULT_BUCKET: &str = "account";

/// `GetAccountRateLimitsResponse`, the fields this build reads.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LimitsAnswer {
    /// The backward-compatible single-bucket view, which mirrors one entry of the map below.
    #[serde(default)]
    pub(crate) rate_limits: Option<Snapshot>,
    /// Every metered bucket, keyed by the CLI's own limit id.
    #[serde(default)]
    pub(crate) rate_limits_by_limit_id: Option<BTreeMap<String, Snapshot>>,
}

/// `RateLimitSnapshot`, the fields this build reads.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    /// The CLI's own id for this bucket.
    #[serde(default)]
    pub(crate) limit_id: Option<String>,
    /// The CLI's own display name for this bucket, which is a model name when the bucket meters one model.
    #[serde(default)]
    pub(crate) limit_name: Option<String>,
    /// The shorter window.
    #[serde(default)]
    pub(crate) primary: Option<LimitWindow>,
    /// The longer one.
    #[serde(default)]
    pub(crate) secondary: Option<LimitWindow>,
    /// The plan this bucket is metered against.
    #[serde(default)]
    pub(crate) plan_type: Option<String>,
    /// Present when a limit has actually been reached, and it names which.
    #[serde(default)]
    pub(crate) rate_limit_reached_type: Option<String>,
    /// The account's spend control is blocking, by the backend's own word.
    #[serde(default)]
    pub(crate) spend_control_reached: Option<bool>,
}

/// `RateLimitWindow`, the fields this build reads.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LimitWindow {
    /// How much of it is used.
    #[serde(default)]
    pub(crate) used_percent: Option<i64>,
    /// How long the window is, in minutes.
    #[serde(default)]
    pub(crate) window_duration_mins: Option<i64>,
    /// When it resets, in unix seconds.
    #[serde(default)]
    pub(crate) resets_at: Option<i64>,
}

impl LimitsAnswer {
    /// One answer holding only the single-bucket view, which is the shape a turn notification arrives in.
    pub(crate) fn from_snapshot(snapshot: Snapshot) -> Self {
        Self {
            rate_limits: Some(snapshot),
            rate_limits_by_limit_id: None,
        }
    }

    /// The buckets to read, each with the key it was filed under.
    ///
    /// The map wins whole when the CLI sends one, because the single field is documented as mirroring an
    /// entry of it: reading both would draw one bucket's windows twice.
    fn buckets(&self) -> Vec<(Option<&str>, &Snapshot)> {
        match self.rate_limits_by_limit_id.as_ref() {
            Some(map) if !map.is_empty() => map
                .iter()
                .map(|(key, snapshot)| (Some(key.as_str()), snapshot))
                .collect(),
            _ => self
                .rate_limits
                .as_ref()
                .map(|snapshot| vec![(None, snapshot)])
                .unwrap_or_default(),
        }
    }

    /// Every window this account has, across every bucket the CLI meters.
    pub(crate) fn windows(&self) -> Vec<Window> {
        let mut windows = Vec::new();
        for (key, snapshot) in self.buckets() {
            let bucket = bucket_id(key, snapshot);
            let label = account_token(snapshot.limit_name.as_deref());
            for (suffix, reported) in [
                ("primary", snapshot.primary.as_ref()),
                ("secondary", snapshot.secondary.as_ref()),
            ] {
                let Some(reported) = reported else { continue };
                let window = window_of(&format!("{bucket}.{suffix}"), label.clone(), reported);
                if window.is_described() {
                    windows.push(window);
                }
            }
        }
        windows
    }

    /// A limit is blocking right now, by this CLI's own word, in any bucket.
    pub(crate) fn reached(&self) -> bool {
        self.buckets().iter().any(|(_, snapshot)| {
            snapshot.rate_limit_reached_type.is_some()
                || snapshot.spend_control_reached.unwrap_or(false)
        })
    }

    /// The plan this account is on, as the first bucket that names one wrote it.
    pub(crate) fn plan(&self) -> Option<Box<str>> {
        self.buckets()
            .iter()
            .find_map(|(_, snapshot)| account_token(snapshot.plan_type.as_deref()))
    }
}

/// The bucket's own id, the key it was filed under, or the default bucket, in that order of authority.
fn bucket_id(key: Option<&str>, snapshot: &Snapshot) -> Box<str> {
    account_token(snapshot.limit_id.as_deref())
        .or_else(|| account_token(key))
        .unwrap_or_else(|| DEFAULT_BUCKET.into())
}

/// One reported window in the shape a gauge draws.
fn window_of(id: &str, label: Option<Box<str>>, reported: &LimitWindow) -> Window {
    Window {
        label,
        used_percent: reported.used_percent.map(|percent| {
            // Capped at a full window, which is how every other driver here reads a percentage. Letting an
            // overrun through as 130 drew a full bar beside the digits `130%`, so one row disagreed with
            // itself; the provider's own overrun stays in the payload for whoever wants it.
            u8::try_from(percent.clamp(0, 100)).unwrap_or(100)
        }),
        resets_at: reported.resets_at.and_then(|seconds| {
            // A negative reset instant is not a time; absent beats a wrong one.
            let Ok(seconds) = u64::try_from(seconds) else {
                return None;
            };
            Some(runtrol_provider::WallMs::from_millis(
                seconds.saturating_mul(1_000),
            ))
        }),
        window_minutes: reported.window_duration_mins.and_then(|minutes| {
            // A length that does not fit is reported as absent rather than clamped into a real number.
            let Ok(minutes) = u32::try_from(minutes) else {
                return None;
            };
            Some(minutes)
        }),
        ..Window::new(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live answer measured on 0.149.1, trimmed to the fields this module reads.
    const MEASURED: &str = r#"{
        "rateLimits": {
            "limitId": "codex", "limitName": null,
            "primary": {"usedPercent": 29, "windowDurationMins": 10080, "resetsAt": 1788300917},
            "secondary": null, "planType": "pro", "rateLimitReachedType": null,
            "spendControlReached": false
        },
        "rateLimitsByLimitId": {
            "codex": {
                "limitId": "codex", "limitName": null,
                "primary": {"usedPercent": 29, "windowDurationMins": 10080, "resetsAt": 1788300917},
                "secondary": null, "planType": "pro", "spendControlReached": false
            },
            "codex_bengalfox": {
                "limitId": "codex_bengalfox", "limitName": "GPT-5.3-Codex-Spark",
                "primary": {"usedPercent": 0, "windowDurationMins": 300, "resetsAt": 1787768152},
                "secondary": {"usedPercent": 0, "windowDurationMins": 10080, "resetsAt": 1788354952},
                "planType": "pro"
            }
        }
    }"#;

    fn measured() -> LimitsAnswer {
        serde_json::from_str(MEASURED).expect("the measured answer parses")
    }

    #[test]
    fn every_metered_bucket_becomes_its_own_window() {
        // The defect this replaced: two slots showed one bucket, so the model-scoped limit that actually
        // stops a turn was invisible.
        let windows = measured().windows();
        let ids: Vec<&str> = windows.iter().map(|window| window.id.as_ref()).collect();
        assert_eq!(
            ids,
            vec![
                "codex.primary",
                "codex_bengalfox.primary",
                "codex_bengalfox.secondary"
            ]
        );
    }

    #[test]
    fn a_model_bucket_carries_the_name_the_cli_gave_it() {
        let windows = measured().windows();
        let spark = windows
            .iter()
            .find(|window| window.id.as_ref() == "codex_bengalfox.primary")
            .expect("the model bucket's shorter window");
        assert_eq!(spark.label.as_deref(), Some("GPT-5.3-Codex-Spark"));
        assert_eq!(spark.window_minutes, Some(300));
        assert_eq!(spark.used_percent, Some(0));
    }

    #[test]
    fn the_reset_instant_is_read_as_seconds() {
        // 1788300917 seconds is six days after the measurement, which is the seven-day window it belongs to.
        // Read as milliseconds it would land three weeks after the epoch.
        let windows = measured().windows();
        let account = windows
            .iter()
            .find(|window| window.id.as_ref() == "codex.primary")
            .expect("the account bucket");
        assert_eq!(
            account.resets_at.map(runtrol_provider::WallMs::as_millis),
            Some(1_788_300_917_000)
        );
    }

    #[test]
    fn the_single_bucket_view_never_doubles_the_map() {
        // The CLI documents `rateLimits` as mirroring one entry of the map. Reading both would draw the
        // account bucket twice, which reads as a limit that exists twice over.
        let windows = measured().windows();
        assert_eq!(
            windows
                .iter()
                .filter(|window| window.id.as_ref() == "codex.primary")
                .count(),
            1
        );
    }

    #[test]
    fn a_turn_notification_carries_only_the_single_bucket() {
        // What arrives unasked mid-turn: one snapshot, no map. It still becomes a window.
        let snapshot: Snapshot = serde_json::from_str(
            r#"{"primary":{"usedPercent":87,"windowDurationMins":300,"resetsAt":1787768152}}"#,
        )
        .expect("the notification's snapshot parses");
        let windows = LimitsAnswer::from_snapshot(snapshot).windows();
        assert_eq!(windows.len(), 1);
        let only = windows.first().expect("the notification's one window");
        assert_eq!(only.id.as_ref(), "account.primary");
        assert_eq!(only.used_percent, Some(87));
    }

    #[test]
    fn a_blocked_bucket_blocks_the_account() {
        let answer: LimitsAnswer = serde_json::from_str(
            r#"{"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":100},"rateLimitReachedType":"rate_limit_reached"}}}"#,
        )
        .expect("parses");
        assert!(answer.reached());
    }

    #[test]
    fn an_empty_window_is_not_drawn() {
        // A window with neither a percentage nor a reset says nothing, and a bar for it would claim a
        // number the CLI never gave.
        let answer: LimitsAnswer =
            serde_json::from_str(r#"{"rateLimits":{"primary":{}}}"#).expect("parses");
        assert!(answer.windows().is_empty());
    }
}
