//! One reading of this CLI's account limits, shared by the two surfaces that publish them.
//!
//! # The two surfaces, and why they need one id space
//!
//! This CLI states its limit position twice. A turn pushes `rate_limit_event` with `unifiedWindows`, and
//! the `get_usage` control request answers the same question without a turn. A gauge merges the two, so a
//! window has to carry the same identity whichever surface reported it, or the same limit draws twice.
//!
//! # What each surface actually says
//!
//! Measured 2026-08-26 on 2.1.246, signed in on `max`.
//!
//! The turn frame keys its windows `five_hour` and `seven_day` and states each utilization as a **fraction
//! of one** (`0.94`), with resets counted in **unix seconds**.
//!
//! The usage answer keys the same two windows by the same names but states utilization as a **percentage**
//! (`92`) with resets written as **ISO 8601 text**, and it adds a self-describing `limits` array the turn
//! frame has no equivalent of:
//!
//! ```text
//! limits: [
//!   { kind: "session",        percent:   3, is_active: false, resets_at: "2026-08-26T18:09:59.801396+00:00" },
//!   { kind: "weekly_all",     percent:  92, is_active: false, resets_at: "2026-08-26T23:59:59.801417+00:00" },
//!   { kind: "weekly_scoped",  percent: 100, is_active: true,  scope: { model: { display_name: "Fable" } } },
//! ]
//! ```
//!
//! That third entry is a whole limit the turn frame never mentions: a week scoped to one model, which is the
//! limit that was actually blocking at the time of the measurement. Reporting only the pair the turn frame
//! knows about showed an account as 92% used while the model it was about to call was refused outright.
//!
//! # Which windows are the account's, by this CLI's own answer
//!
//! Read out of the CLI's own `/usage` renderer rather than chosen here: it lists `five_hour` as "Current
//! session", `seven_day` as "Current week (all models)", `seven_day_sonnet` as "Current week (Sonnet only)"
//! on the plans that have it, and then every `weekly_scoped` entry as "Current week (<model>)". The other
//! keys the answer carries are not in that list and are not drawn, which is why an account with a
//! `nimbus_quill` bucket at zero does not grow a bar nobody can act on.

use std::collections::BTreeMap;

use runtrol_provider::{WallMs, Window, account_token};
use serde::Deserialize;

/// The account's short window, by the name both surfaces give it.
const FIVE_HOUR: &str = "five_hour";
/// The account's whole-account week, by the name both surfaces give it.
const SEVEN_DAY: &str = "seven_day";
/// The identity of a limit reported with no window name at all, which older builds did.
const GOVERNING: &str = "governing";

/// How long a window is, from the name this CLI gives it.
///
/// A rule over the vendor's own naming rather than a table of every key: measured, this CLI names a window
/// by its length and then suffixes a scope (`seven_day`, `seven_day_opus`, `seven_day_sonnet`). A name that
/// states no length claims none, and the strip then labels that bar with whatever the provider scoped it to.
fn minutes_of(id: &str) -> Option<u32> {
    if id.starts_with(FIVE_HOUR) {
        return Some(300);
    }
    if id.starts_with(SEVEN_DAY) {
        return Some(10_080);
    }
    None
}

/// A fraction of one as the whole percent a bar can draw.
///
/// Clamped rather than trusted: the bar is a proportion, and a value outside the range would either
/// overflow the cast or draw a bar longer than its track.
pub(crate) fn percent_of_fraction(fraction: f64) -> u8 {
    percent_of(fraction * 100.0)
}

/// A percentage this CLI already stated, as the whole percent a bar can draw.
fn percent_of(percent: f64) -> u8 {
    let scaled = percent.round();
    if scaled.is_nan() || scaled <= 0.0 {
        return 0;
    }
    if scaled >= 100.0 {
        return 100;
    }
    // In range and finite by the two guards above, so this cast is exact rather than truncating.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let whole = scaled as u8;
    whole
}

/// One window inside the turn frame's `unifiedWindows`.
#[derive(Deserialize)]
pub(crate) struct UnifiedWindow {
    /// How full that window is, as a fraction from zero to one.
    #[serde(default)]
    pub(crate) utilization: Option<f64>,
    /// When that window resets, in unix seconds.
    #[serde(default, rename = "resetsAt")]
    pub(crate) resets_at: Option<u64>,
}

/// The windows a turn reported, shortest first.
pub(crate) fn from_unified(reported: &BTreeMap<String, UnifiedWindow>) -> Vec<Window> {
    reported
        .iter()
        .filter_map(|(name, window)| {
            let id = account_token(Some(name))?;
            let drawn = Window {
                used_percent: window.utilization.map(percent_of_fraction),
                resets_at: window
                    .resets_at
                    .map(|seconds| WallMs::from_millis(seconds.saturating_mul(1_000))),
                window_minutes: minutes_of(&id),
                ..Window::new(&id)
            };
            drawn.is_described().then_some(drawn)
        })
        .collect()
}

/// The one window an older build reported, which named no window at all.
pub(crate) fn governing_only(utilization: Option<f64>, resets_at: Option<u64>) -> Option<Window> {
    let window = Window {
        used_percent: utilization.map(percent_of_fraction),
        resets_at: resets_at.map(|seconds| WallMs::from_millis(seconds.saturating_mul(1_000))),
        governing: true,
        ..Window::new(GOVERNING)
    };
    window.is_described().then_some(window)
}

/// The `get_usage` control answer, the fields this build reads.
#[derive(Default, Deserialize)]
pub(crate) struct UsageAnswer {
    /// The plan this account is on, in this CLI's own token.
    #[serde(default)]
    pub(crate) subscription_type: Option<String>,
    /// False when plan limits do not apply at all (an API key, a third-party provider, a missing scope).
    #[serde(default)]
    pub(crate) rate_limits_available: bool,
    /// The windows themselves, absent when the account has none to state.
    #[serde(default)]
    pub(crate) rate_limits: Option<UsageLimits>,
}

/// The `rate_limits` object, the fields this build reads.
///
/// Keys this build does not name are ignored on purpose. The answer carries buckets an account may have no
/// business seeing, and the CLI's own renderer draws exactly the ones named here.
#[derive(Default, Deserialize)]
pub(crate) struct UsageLimits {
    /// The five-hour window, as a percentage.
    #[serde(default)]
    five_hour: Option<Reading>,
    /// The whole-account week, as a percentage.
    #[serde(default)]
    seven_day: Option<Reading>,
    /// This CLI's older per-model week, on the plans that still have one.
    #[serde(default)]
    seven_day_opus: Option<Reading>,
    /// The same, for its other named model.
    #[serde(default)]
    seven_day_sonnet: Option<Reading>,
    /// The self-describing view, which is where model-scoped windows arrive.
    #[serde(default)]
    limits: Vec<LimitEntry>,
}

/// One named window in the usage answer.
#[derive(Deserialize)]
struct Reading {
    /// Percentage of the window used, zero to one hundred.
    #[serde(default)]
    utilization: Option<f64>,
    /// When the window resets, as ISO 8601 text.
    #[serde(default)]
    resets_at: Option<String>,
}

/// One entry of the self-describing `limits` array.
#[derive(Deserialize)]
struct LimitEntry {
    /// Which kind of limit this row is.
    #[serde(default)]
    kind: Option<String>,
    /// How much of it is used, zero to one hundred.
    #[serde(default)]
    percent: Option<f64>,
    /// When it resets, as ISO 8601 text.
    #[serde(default)]
    resets_at: Option<String>,
    /// This CLI's own word for "this is the limit governing right now".
    #[serde(default)]
    is_active: bool,
    /// What the limit is scoped to, on the rows that are scoped.
    #[serde(default)]
    scope: Option<LimitScope>,
}

/// The scope of one limit row.
#[derive(Deserialize)]
struct LimitScope {
    /// The model, on the rows scoped to one.
    #[serde(default)]
    model: Option<ScopeModel>,
}

/// The model one limit row is scoped to.
#[derive(Deserialize)]
struct ScopeModel {
    /// The server's own label for it, which is the name a person recognises.
    #[serde(default)]
    display_name: Option<String>,
}

impl LimitEntry {
    /// The model this row is scoped to, when it is scoped to one.
    fn model(&self) -> Option<Box<str>> {
        account_token(self.scope.as_ref()?.model.as_ref()?.display_name.as_deref())
    }

    /// Which window this row is talking about, in the shared id space.
    ///
    /// The array names the same two windows the rest of the answer keys as `five_hour` and `seven_day`:
    /// measured, its `session` row carried the same percentage and the same reset instant as `five_hour`,
    /// and its `weekly_all` row matched `seven_day`. One id per window, so the array's word on which limit
    /// is governing lands on the window it is about instead of creating a second copy of it.
    fn window_id(&self) -> Option<Box<str>> {
        match self.kind.as_deref()? {
            "session" => Some(FIVE_HOUR.into()),
            "weekly_all" => Some(SEVEN_DAY.into()),
            "weekly_scoped" => self
                .model()
                .map(|model| format!("{SEVEN_DAY}:{model}").into()),
            _ => None,
        }
    }
}

impl UsageAnswer {
    /// Every window this account has, by this CLI's own answer.
    pub(crate) fn windows(&self) -> Vec<Window> {
        let Some(limits) = self.rate_limits.as_ref() else {
            return Vec::new();
        };
        let mut windows: Vec<Window> = [
            (FIVE_HOUR, None, limits.five_hour.as_ref()),
            (SEVEN_DAY, None, limits.seven_day.as_ref()),
            // This CLI's own renderer still reads these two by name, so a plan that reports one keeps its
            // bar. The scope is the model token out of the key, in the vendor's own spelling: capitalising
            // it here would be runtrol naming somebody else's model.
            (
                "seven_day_opus",
                Some("opus"),
                limits.seven_day_opus.as_ref(),
            ),
            (
                "seven_day_sonnet",
                Some("sonnet"),
                limits.seven_day_sonnet.as_ref(),
            ),
        ]
        .into_iter()
        .filter_map(|(id, scope, reading)| {
            let reading = reading?;
            let window = Window {
                scope: scope.map(Into::into),
                used_percent: reading.utilization.map(percent_of),
                resets_at: reading.resets_at.as_deref().and_then(WallMs::from_iso8601),
                window_minutes: minutes_of(id),
                ..Window::new(id)
            };
            window.is_described().then_some(window)
        })
        .collect();

        // The model-scoped weeks, which exist only in the array. Its own `weekly_scoped` rows are the
        // general mechanism the two named keys above predate.
        for entry in &limits.limits {
            if entry.kind.as_deref() != Some("weekly_scoped") {
                continue;
            }
            let Some(model) = entry.model() else { continue };
            let id = format!("{SEVEN_DAY}:{model}");
            let window = Window {
                scope: Some(model),
                used_percent: entry.percent.map(percent_of),
                resets_at: entry.resets_at.as_deref().and_then(WallMs::from_iso8601),
                window_minutes: minutes_of(&id),
                governing: entry.is_active,
                ..Window::new(&id)
            };
            if window.is_described() {
                windows.push(window);
            }
        }

        // The array's word on which limit is binding, applied to whichever window it names.
        for entry in &limits.limits {
            if !entry.is_active {
                continue;
            }
            let Some(id) = entry.window_id() else {
                continue;
            };
            for window in &mut windows {
                if window.id == id {
                    window.governing = true;
                }
            }
        }
        windows
    }

    /// A limit is blocking right now.
    ///
    /// This answer states no blocked flag of its own, so the reading is the one fact it does state: a
    /// window the service reports as entirely used is the window it refuses on. Measured against the same
    /// account at the same moment, the model-scoped week read 100 while that model's requests were refused.
    pub(crate) fn reached(&self) -> bool {
        self.windows()
            .iter()
            .any(|window| window.used_percent == Some(100))
    }

    /// The plan this account is on, in this CLI's own token.
    pub(crate) fn plan(&self) -> Option<Box<str>> {
        account_token(self.subscription_type.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live answer measured on 2.1.246, trimmed to the fields this module reads and keeping two of the
    /// buckets it deliberately ignores.
    const MEASURED: &str = r#"{
      "subscription_type": "max",
      "rate_limits_available": true,
      "rate_limits": {
        "five_hour": {"utilization": 3, "resets_at": "2026-08-26T18:09:59.801396+00:00"},
        "seven_day": {"utilization": 92, "resets_at": "2026-08-26T23:59:59.801417+00:00"},
        "seven_day_opus": null,
        "seven_day_sonnet": null,
        "nimbus_quill": {"utilization": 0, "resets_at": null},
        "tangelo": null,
        "limits": [
          {"kind": "session", "group": "session", "percent": 3, "severity": "normal",
           "resets_at": "2026-08-26T18:09:59.801396+00:00", "scope": null, "is_active": false},
          {"kind": "weekly_all", "group": "weekly", "percent": 92, "severity": "critical",
           "resets_at": "2026-08-26T23:59:59.801417+00:00", "scope": null, "is_active": false},
          {"kind": "weekly_scoped", "group": "weekly", "percent": 100, "severity": "critical",
           "resets_at": "2026-08-26T23:59:59.801688+00:00",
           "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}, "is_active": true}
        ],
        "model_scoped": [
          {"display_name": "Fable", "utilization": 100, "resets_at": "2026-08-26T23:59:59.801688+00:00"}
        ]
      }
    }"#;

    fn measured() -> UsageAnswer {
        serde_json::from_str(MEASURED).expect("the measured answer parses")
    }

    #[test]
    fn the_three_windows_the_account_actually_has_all_arrive() {
        // The operator named these three by hand: the five-hour one, the whole-account week, and the week
        // scoped to one model. Two slots could carry at most the first two.
        let windows = measured().windows();
        let ids: Vec<&str> = windows.iter().map(|window| window.id.as_ref()).collect();
        assert_eq!(ids, vec!["five_hour", "seven_day", "seven_day:Fable"]);
    }

    #[test]
    fn the_model_scoped_week_carries_the_model_the_server_named() {
        let windows = measured().windows();
        let fable = windows
            .iter()
            .find(|window| window.id.as_ref() == "seven_day:Fable")
            .expect("the model-scoped week");
        assert_eq!(fable.scope.as_deref(), Some("Fable"));
        assert_eq!(fable.used_percent, Some(100));
        assert_eq!(fable.window_minutes, Some(10_080));
        assert!(
            fable.governing,
            "the answer marks this one as the limit governing right now"
        );
    }

    #[test]
    fn a_bucket_the_cli_does_not_draw_is_not_drawn_here_either() {
        // `nimbus_quill` reports a real zero, and a bar for it would be a limit nobody can act on with a
        // name nobody can read. The CLI's own renderer lists neither it nor `tangelo`.
        let ids: Vec<Box<str>> = measured()
            .windows()
            .into_iter()
            .map(|window| window.id)
            .collect();
        assert!(!ids.iter().any(|id| id.as_ref() == "nimbus_quill"));
    }

    #[test]
    fn the_percentages_are_read_as_percentages_here_and_fractions_on_a_turn() {
        // The two surfaces state the same window on different scales. Reading either one the other way
        // turns 92% into 9200% or into 1%.
        let windows = measured().windows();
        let week = windows
            .iter()
            .find(|window| window.id.as_ref() == SEVEN_DAY)
            .expect("the week");
        assert_eq!(week.used_percent, Some(92));

        let turn = from_unified(&BTreeMap::from([(
            SEVEN_DAY.to_owned(),
            UnifiedWindow {
                utilization: Some(0.92),
                resets_at: Some(1_787_788_799),
            },
        )]));
        assert_eq!(
            turn.first().and_then(|window| window.used_percent),
            Some(92)
        );
    }

    #[test]
    fn both_surfaces_name_the_same_window_the_same_way() {
        // The whole point of one id space: a probe's reading and a turn's reading of the same window
        // replace each other in the gauge instead of drawing two bars for one limit.
        let probed: Vec<Box<str>> = measured()
            .windows()
            .into_iter()
            .map(|window| window.id)
            .collect();
        let turned = from_unified(&BTreeMap::from([
            (
                FIVE_HOUR.to_owned(),
                UnifiedWindow {
                    utilization: Some(0.03),
                    resets_at: None,
                },
            ),
            (
                SEVEN_DAY.to_owned(),
                UnifiedWindow {
                    utilization: Some(0.92),
                    resets_at: None,
                },
            ),
        ]));
        for window in turned {
            assert!(
                probed.contains(&window.id),
                "{} is named differently by the two surfaces",
                window.id
            );
        }
    }

    #[test]
    fn an_exhausted_window_is_a_blocking_limit() {
        assert!(measured().reached());
    }

    #[test]
    fn an_account_with_no_plan_limits_reports_no_windows() {
        // An API key or a third-party provider: the answer says so instead of showing an empty account.
        let answer: UsageAnswer = serde_json::from_str(
            r#"{"subscription_type":null,"rate_limits_available":false,"rate_limits":null}"#,
        )
        .expect("parses");
        assert!(!answer.rate_limits_available);
        assert!(answer.windows().is_empty());
        assert!(!answer.reached());
        assert!(answer.plan().is_none());
    }
}
