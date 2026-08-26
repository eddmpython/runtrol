//! Where the operator's account with one ACP agent stands, read from what its manifest declares.
//!
//! # Why none of this names a vendor
//!
//! The standard protocol says nothing about accounts. Every agent that publishes one does it through its own
//! extension, so the honest choices were a branch per agent in the shared driver or a declaration per agent
//! in its manifest. The first is the thing the core is forbidden to grow; this is the second. What arrives
//! here is a method name and a set of JSON Pointers, and nothing in this file knows whose they are.
//!
//! # What that buys, measured
//!
//! Grok 1.0.5, asked over its own stdio channel with no session and no turn (2026-08-26):
//!
//! ```text
//! _x.ai/auth/check_subscription -> {"authenticated": true,
//!                                   "meta": {"auth_mode": "Oidc", "subscription_tier": "SuperGrok"}}
//! _x.ai/billing                 -> {"config": {"billingPeriodStart": "2026-08-23T22:02:11.135977+00:00",
//!                                              "billingPeriodEnd":   "2026-08-30T22:02:11.135977+00:00",
//!                                              "onDemandCap": {"val": 0}, "onDemandUsed": {"val": 0}},
//!                                   "subscription_tier": "SuperGrok"}
//! ```
//!
//! Before this, that agent's row said "No usage published" because the generic driver never overrode the
//! default report. It publishes a plan, a sign-in state and a billing period, and now says all three.
//!
//! # What is deliberately not derived
//!
//! A percentage this account did not state. That answer carries `creditUsagePercent` only for a plan that
//! meters credits, and computing one from a cap of zero would draw a full bar on an account with no meter.
//! The window still draws its reset, which is a real thing to know, and the bar appears the day the agent
//! publishes the number.

use std::collections::BTreeMap;

use runtrol_childproc::{Containment, Program};
use runtrol_provider::{
    AccountIdentitySpec, AccountLimits, AccountReport, AccountStatus, AccountUnmeteredSpec,
    AccountWindowSpec, LimitsAbsent, ProviderError, ProviderId, WallMs, Window, account_token,
};

use crate::acp::scratch::ScratchConnection;

/// The account this agent reports, or a report saying its manifest declares nothing to ask.
///
/// A failure to read the limits does not fail the whole answer: sign-in state is a separate fact and still
/// true, and a row that lost both would call a working service unpublished.
pub(super) async fn read(
    provider: ProviderId,
    program: &Program,
    transport_argv: &[Box<str>],
    identity: &AccountIdentitySpec,
    windows: &[AccountWindowSpec],
    unmetered: Option<&AccountUnmeteredSpec>,
    contained_by: &Containment,
) -> Result<AccountReport, ProviderError> {
    let mut connection =
        ScratchConnection::start(provider, program, transport_argv, None, contained_by)?;
    let outcome = async {
        connection.initialized().await?;
        let answer = ask(&mut connection, &identity.method).await?;
        let mut report = identity_of(identity, &answer);
        if matches!(report.status, AccountStatus::SignedIn) && !windows.is_empty() {
            match read_windows(&mut connection, windows).await {
                Ok(drawn) if drawn.is_empty() => {
                    report.limits_absent = Some(LimitsAbsent::Unmetered {
                        why: "this account publishes no limit numbers".into(),
                    });
                }
                Ok(drawn) => {
                    // A window with a reset and no percentage is a period the operator cannot see the
                    // fill of. The agent has its own word for why, and saying it beats a bar-less row
                    // that looks like something went wrong.
                    let metered = drawn.iter().any(|window| window.used_percent.is_some());
                    report.limits = Some(AccountLimits::new(drawn, false));
                    if !metered {
                        report.limits_absent =
                            unmetered_reason(&mut connection, unmetered, &identity.method, &answer)
                                .await;
                    }
                }
                Err(why) => report.limits_absent = Some(LimitsAbsent::Unread { why: why.into() }),
            }
        }
        Ok(report)
    }
    .await;
    let cleanup = connection.close().await;
    match (outcome, cleanup) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(report), Ok(())) => Ok(report),
    }
}

/// The agent's own word for why this account shows no numbers, when it has one and it applies.
///
/// Asked of the identity answer first, because that is already in hand: an agent that keeps the fact
/// beside who is signed in costs no second call for it.
async fn unmetered_reason(
    connection: &mut ScratchConnection,
    spec: Option<&AccountUnmeteredSpec>,
    identity_method: &str,
    identity_answer: &serde_json::Value,
) -> Option<LimitsAbsent> {
    let spec = spec?;
    let answer = if spec.method.as_ref() == identity_method {
        identity_answer.clone()
    } else {
        match ask(connection, &spec.method).await {
            Ok(answer) => answer,
            // The reason is an explanation, never the reading itself. A method that did not answer costs
            // the sentence and nothing else, and the row still shows the period it did read.
            Err(_) => return None,
        }
    };
    let holds = match answer.pointer(&spec.when) {
        None | Some(serde_json::Value::Null | serde_json::Value::Bool(false)) => false,
        Some(serde_json::Value::String(text)) => !text.trim().is_empty(),
        Some(_) => true,
    };
    holds.then(|| LimitsAbsent::Unmetered {
        why: spec.say.clone(),
    })
}

/// One extension call with no parameters, answered as a value the pointers can walk.
///
/// No parameters because every declared method here answers about the account rather than about anything
/// the caller chooses, and inventing an argument shape would be a second thing to get wrong per agent.
async fn ask(
    connection: &mut ScratchConnection,
    method: &str,
) -> Result<serde_json::Value, ProviderError> {
    let provider = connection.provider;
    let answer = connection
        .call(
            method,
            &serde_json::json!({}),
            "asking the agent about the account",
        )
        .await?;
    serde_json::from_slice(&answer).map_err(|error| ProviderError::Protocol {
        provider,
        doing: "asking the agent about the account",
        detail: error.to_string(),
    })
}

/// The sign-in state and plan the declared pointers find.
fn identity_of(spec: &AccountIdentitySpec, answer: &serde_json::Value) -> AccountReport {
    let signed_in = answer
        .pointer(&spec.signed_in)
        .and_then(serde_json::Value::as_bool);
    let status = match signed_in {
        Some(true) => AccountStatus::SignedIn,
        Some(false) => AccountStatus::SignedOut,
        // The method answered and the declared place held no yes or no. That is the manifest and the agent
        // disagreeing about the answer's shape, which is a thing to say rather than a state to invent.
        None => {
            return AccountReport::unpublished(
                "the agent's account answer had nothing where its manifest says the sign-in state is",
            );
        }
    };
    AccountReport {
        status,
        plan: token_at(answer, spec.plan.as_deref()),
        method: token_at(answer, spec.via.as_deref()),
        limits: None,
        limits_absent: None,
        tokens_today: None,
    }
}

/// Every declared window, asking each method once however many windows read from it.
///
/// Grouped by method rather than asked per window, because two windows can be two numbers in one answer and
/// a second call would be a second process round trip for bytes already in hand.
async fn read_windows(
    connection: &mut ScratchConnection,
    specs: &[AccountWindowSpec],
) -> Result<Vec<Window>, String> {
    let mut answers: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
    for spec in specs {
        if answers.contains_key(&*spec.method) {
            continue;
        }
        let answer = ask(connection, &spec.method)
            .await
            .map_err(|error| format!("the agent did not answer about its limits: {error}"))?;
        drop(answers.insert(&spec.method, answer));
    }
    let mut drawn = Vec::new();
    for spec in specs {
        let Some(answer) = answers.get(&*spec.method) else {
            continue;
        };
        let window = window_of(spec, answer);
        if window.is_described() {
            drawn.push(window);
        }
    }
    Ok(drawn)
}

/// One window, from wherever its declaration says each number lives.
fn window_of(spec: &AccountWindowSpec, answer: &serde_json::Value) -> Window {
    let opened = instant_at(answer, spec.starts_at.as_deref());
    let resets_at = instant_at(answer, spec.resets_at.as_deref());
    Window {
        used_percent: answer
            .pointer(spec.used_percent.as_deref().unwrap_or(""))
            .and_then(serde_json::Value::as_f64)
            .map(percent_of),
        resets_at,
        // The agent's own two instants, not a length anybody declared: an account billed monthly and one
        // billed weekly answer the same method, and the difference between what it said is the only reading
        // that is right for both.
        window_minutes: opened
            .zip(resets_at)
            .and_then(|(opened, resets)| opened.millis_until(resets))
            .and_then(|millis| {
                // A period longer than eight thousand years is not a period; absent beats a wrong length.
                let Ok(minutes) = u32::try_from(millis / 60_000) else {
                    return None;
                };
                Some(minutes)
            })
            .filter(|minutes| *minutes > 0),
        ..Window::new(&spec.id)
    }
}

/// A bounded plain token at a declared pointer, or nothing.
fn token_at(answer: &serde_json::Value, pointer: Option<&str>) -> Option<Box<str>> {
    account_token(answer.pointer(pointer?)?.as_str())
}

/// An instant at a declared pointer, however the agent chose to write it.
///
/// Both spellings are in the wild and neither is discoverable, so both are read: text is ISO 8601 and a
/// number is unix seconds. Anything else is absent rather than a guessed century.
fn instant_at(answer: &serde_json::Value, pointer: Option<&str>) -> Option<WallMs> {
    match answer.pointer(pointer?)? {
        serde_json::Value::String(text) => WallMs::from_iso8601(text),
        serde_json::Value::Number(number) => number
            .as_u64()
            .map(|seconds| WallMs::from_millis(seconds.saturating_mul(1_000))),
        _ => None,
    }
}

/// A percentage an agent stated, as the whole percent a bar can draw.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_spec() -> AccountIdentitySpec {
        AccountIdentitySpec {
            method: "_x.ai/auth/check_subscription".into(),
            signed_in: "/authenticated".into(),
            plan: Some("/meta/subscription_tier".into()),
            via: Some("/meta/auth_mode".into()),
        }
    }

    fn window_spec() -> AccountWindowSpec {
        AccountWindowSpec {
            method: "_x.ai/billing".into(),
            id: "billing_period".into(),
            used_percent: Some("/config/creditUsagePercent".into()),
            starts_at: Some("/config/billingPeriodStart".into()),
            resets_at: Some("/config/billingPeriodEnd".into()),
        }
    }

    /// The live answers measured on grok 1.0.5, 2026-08-26.
    fn measured_identity() -> serde_json::Value {
        serde_json::json!({
            "authenticated": true,
            "meta": {"auth_mode": "Oidc", "subscription_tier": "SuperGrok", "is_zdr": false}
        })
    }

    fn measured_billing() -> serde_json::Value {
        serde_json::json!({
            "config": {
                "currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY"},
                "onDemandCap": {"val": 0},
                "onDemandUsed": {"val": 0},
                "billingPeriodStart": "2026-08-23T22:02:11.135977+00:00",
                "billingPeriodEnd": "2026-08-30T22:02:11.135977+00:00"
            },
            "subscription_tier": "SuperGrok"
        })
    }

    #[test]
    fn the_declared_pointers_find_the_account() {
        // Before this the same agent reported nothing at all, because the generic driver had no way to be
        // told where its answer keeps things.
        let report = identity_of(&identity_spec(), &measured_identity());
        assert_eq!(report.status, AccountStatus::SignedIn);
        assert_eq!(report.plan.as_deref(), Some("SuperGrok"));
        assert_eq!(report.method.as_deref(), Some("Oidc"));
    }

    #[test]
    fn the_window_length_comes_from_the_agent_s_own_two_instants() {
        // Seven days, because that is what the agent said its period is. Nothing here declares a length,
        // so an account billed monthly reads as a month on the same declaration.
        let window = window_of(&window_spec(), &measured_billing());
        assert_eq!(window.id.as_ref(), "billing_period");
        assert_eq!(window.window_minutes, Some(10_080));
        assert!(window.resets_at.is_some());
    }

    #[test]
    fn a_percentage_the_account_never_stated_is_not_invented() {
        // This plan meters no credits, so the answer carries no `creditUsagePercent`. The window keeps its
        // reset and grows a bar the day the agent publishes a number.
        let window = window_of(&window_spec(), &measured_billing());
        assert_eq!(window.used_percent, None);
        assert!(
            window.is_described(),
            "a reset alone is still worth drawing"
        );
    }

    #[test]
    fn a_plan_that_meters_credits_draws_its_bar() {
        let mut answer = measured_billing();
        answer
            .get_mut("config")
            .and_then(serde_json::Value::as_object_mut)
            .expect("the measured answer has a config object")
            .insert("creditUsagePercent".to_owned(), serde_json::json!(37.4));
        assert_eq!(window_of(&window_spec(), &answer).used_percent, Some(37));
    }

    #[test]
    fn a_signed_out_agent_is_signed_out_and_not_unpublished() {
        let answer = serde_json::json!({"authenticated": false, "meta": {}});
        let report = identity_of(&identity_spec(), &answer);
        assert_eq!(report.status, AccountStatus::SignedOut);
        assert!(report.plan.is_none());
    }

    #[test]
    fn a_manifest_pointing_at_nothing_says_so_rather_than_guessing() {
        // The agent answered and the declared place was empty, which means the declaration and the agent
        // disagree. Reading that as signed out would show a signed-in operator a sign-in button.
        let answer = serde_json::json!({"somethingElse": true});
        assert!(matches!(
            identity_of(&identity_spec(), &answer).status,
            AccountStatus::Unpublished { .. }
        ));
    }

    #[test]
    fn an_undescribed_window_is_not_drawn() {
        let answer = serde_json::json!({"config": {}});
        assert!(!window_of(&window_spec(), &answer).is_described());
    }
}
