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
    // The prefix has to end where the name ends or where a suffix begins, or a key this build has never
    // seen takes a length from a coincidence: `five_hourly` read as five hours.
    let named = |prefix: &str| {
        id.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '_']))
    };
    if named(FIVE_HOUR) {
        return Some(300);
    }
    if named(SEVEN_DAY) {
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
    /// and its `weekly_all` row matched `seven_day`. One id per window, so a turn's reading and this one
    /// replace each other in the gauge instead of drawing two bars for one limit.
    ///
    /// A kind this build has never seen keeps its own name, and its scope when it has one. That is not a
    /// guess about what the limit means: it is the row appearing with whatever the service called it,
    /// which is the only thing that is true about it here.
    fn window_id(&self) -> Option<Box<str>> {
        let kind = account_token(self.kind.as_deref())?;
        let base: Box<str> = match kind.as_ref() {
            "session" => FIVE_HOUR.into(),
            "weekly_all" | "weekly_scoped" => SEVEN_DAY.into(),
            _ => kind,
        };
        Some(match self.model() {
            Some(model) => format!("{base}:{model}").into(),
            None => base,
        })
    }

    /// This row as a window, when it described one.
    fn window(&self) -> Option<Window> {
        let id = self.window_id()?;
        let window = Window {
            scope: self.model(),
            used_percent: self.percent.map(percent_of),
            resets_at: self.resets_at.as_deref().and_then(WallMs::from_iso8601),
            window_minutes: minutes_of(&id),
            governing: self.is_active,
            ..Window::new(&id)
        };
        window.is_described().then_some(window)
    }
}

impl UsageAnswer {
    /// Every window this account has, by this CLI's own answer.
    pub(crate) fn windows(&self) -> Vec<Window> {
        let Some(limits) = self.rate_limits.as_ref() else {
            return Vec::new();
        };
        // The array is this CLI's own answer to "which limits does this account have": its `/usage` screen
        // is drawn from it, and it is where a window that no key names arrives. Read whole rather than
        // filtered to the kinds this build recognises, because a kind nobody here has seen is exactly the
        // one a silent filter would drop, and it would be dropped on the day the vendor added it.
        let mut windows: Vec<Window> = limits
            .limits
            .iter()
            .filter_map(LimitEntry::window)
            .collect();
        // The named keys, for a build that sends no array and for a window the array left out. Same ids, so
        // an account whose array already described a window keeps the array's reading of it.
        for (id, scope, reading) in [
            (FIVE_HOUR.to_owned(), None, limits.five_hour.as_ref()),
            (SEVEN_DAY.to_owned(), None, limits.seven_day.as_ref()),
            // This CLI's older per-model keys. They carry the identity the array's `weekly_scoped` rows
            // carry, because they are one limit said twice: an account reporting both used to grow two
            // bars for one model's week, and the list keeps the first reading of an identity. The scope is
            // the model token out of the key in the vendor's own spelling, because capitalising it would
            // be runtrol naming somebody else's model.
            (
                format!("{SEVEN_DAY}:opus"),
                Some("opus"),
                limits.seven_day_opus.as_ref(),
            ),
            (
                format!("{SEVEN_DAY}:sonnet"),
                Some("sonnet"),
                limits.seven_day_sonnet.as_ref(),
            ),
        ] {
            let Some(reading) = reading else { continue };
            let window = Window {
                scope: scope.map(Into::into),
                used_percent: reading.utilization.map(percent_of),
                resets_at: reading.resets_at.as_deref().and_then(WallMs::from_iso8601),
                window_minutes: minutes_of(&id),
                ..Window::new(&id)
            };
            if window.is_described() && !window.is_known_in(&windows) {
                windows.push(window);
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
    fn a_limit_kind_this_build_has_never_seen_still_draws() {
        // The array is the service's own list of what this account is limited by, so a filter that kept
        // only the kinds named in this file would drop exactly the window the vendor added last week. It
        // arrives under whatever the service called it, which is the only thing true about it here.
        let answer: UsageAnswer = serde_json::from_str(
            r#"{"rate_limits":{"limits":[
                {"kind":"monthly_overage","percent":42,"resets_at":"2026-09-01T00:00:00Z","is_active":false}
            ]}}"#,
        )
        .expect("parses");
        let windows = answer.windows();
        assert_eq!(windows.len(), 1);
        let unknown = windows.first().expect("the one row the array carried");
        assert_eq!(unknown.id.as_ref(), "monthly_overage");
        assert_eq!(unknown.used_percent, Some(42));
        assert_eq!(
            unknown.window_minutes, None,
            "its name states no length, so none is claimed"
        );
    }

    #[test]
    fn the_array_and_the_named_keys_never_draw_one_window_twice() {
        // Both describe `five_hour`. Measured, they agree; either way one bar comes out, and it is the
        // array's reading because that is the one carrying whether the limit is binding.
        let windows = measured().windows();
        assert_eq!(
            windows
                .iter()
                .filter(|window| window.id.as_ref() == "five_hour")
                .count(),
            1
        );
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
