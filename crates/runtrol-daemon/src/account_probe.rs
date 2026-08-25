//! Asking each installed coding service where the operator's account stands, on the service's own
//! status surface, and remembering the answer for the provider and usage projections.
//!
//! Runs at daemon start and then on a slow clock. Nothing here reads a credential file or a transcript:
//! a driver either has a published status surface (`claude auth status --json`, Codex `account/read`)
//! or says it has none, and the projections repeat exactly that.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use runtrol_provider::{AccountReport, AccountStatus, ProviderId, WallMs};
use tokio::sync::watch;

use crate::Composed;

/// How long one service may take to answer. A status command reads a file; a protocol read is one round trip.
const ACCOUNT_PROBE_DEADLINE: Duration = Duration::from_secs(20);
/// How long after start the first round runs: after the window has drawn, before anyone wonders.
const FIRST_ROUND_DELAY: Duration = Duration::from_secs(2);
/// How often the round repeats. Sign-in changes are rare and a limit window moves slowly.
const ROUND_INTERVAL: Duration = Duration::from_mins(10);

/// One service's latest report and when it arrived.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Reported {
    pub(crate) report: AccountReport,
    pub(crate) at: WallMs,
}

/// Every service's latest report.
#[derive(Debug, Default)]
pub(crate) struct AccountReports {
    latest: BTreeMap<ProviderId, Reported>,
}

impl AccountReports {
    /// Remember one answer. The newest always wins: the report answers "now".
    pub(crate) fn record(&mut self, provider: ProviderId, report: AccountReport, at: WallMs) {
        self.latest.insert(provider, Reported { report, at });
    }

    pub(crate) fn get(&self, provider: ProviderId) -> Option<&Reported> {
        self.latest.get(&provider)
    }

    /// The public shape of one report, for the provider descriptor.
    pub(crate) fn descriptor(
        &self,
        provider: ProviderId,
    ) -> Option<runtrol_runtime_protocol::ProviderAccount> {
        let reported = self.get(provider)?;
        let (status, why) = match &reported.report.status {
            AccountStatus::SignedIn => (
                runtrol_runtime_protocol::ProviderAccountStatus::SignedIn,
                None,
            ),
            AccountStatus::SignedOut => (
                runtrol_runtime_protocol::ProviderAccountStatus::SignedOut,
                None,
            ),
            AccountStatus::Unpublished { why } => (
                runtrol_runtime_protocol::ProviderAccountStatus::Unpublished,
                Some(why.to_string()),
            ),
            // A status kind a newer driver crate added: said as unaskable rather than guessed either way.
            _ => (
                runtrol_runtime_protocol::ProviderAccountStatus::Unpublished,
                Some(
                    "the driver answered with an account status this build does not know"
                        .to_owned(),
                ),
            ),
        };
        Some(runtrol_runtime_protocol::ProviderAccount {
            status,
            plan: reported.report.plan.as_ref().map(ToString::to_string),
            method: reported.report.method.as_ref().map(ToString::to_string),
            why,
            checked_at_ms: reported.at.as_millis(),
        })
    }

    /// Every service whose report carried limit windows, as gauges the usage list can merge.
    pub(crate) fn probed_gauges(&self) -> Vec<runtrol_core::ProviderGauge> {
        self.latest
            .iter()
            .filter_map(|(provider, reported)| {
                let limit = reported.report.as_rate_limit()?;
                Some(runtrol_core::ProviderGauge {
                    provider: *provider,
                    reached: limit.reached,
                    primary: limit.primary,
                    secondary: limit.secondary,
                    cost: None,
                    at: reported.at,
                })
            })
            .collect()
    }
}

/// Ask every usable service, at start and on the clock, and republish what changed.
pub(crate) async fn supervise(
    composed: Arc<Composed>,
    providers: watch::Sender<Arc<runtrol_runtime_protocol::ProviderList>>,
    usage: watch::Sender<Arc<runtrol_runtime_protocol::ProviderUsageList>>,
) {
    tokio::time::sleep(FIRST_ROUND_DELAY).await;
    loop {
        round(&composed, &providers, &usage).await;
        tokio::time::sleep(ROUND_INTERVAL).await;
    }
}

/// One round over every usable service. A service that does not answer gets an unpublished report
/// naming that, never a stale green light and never a silent blank.
pub(crate) async fn round(
    composed: &Arc<Composed>,
    providers: &watch::Sender<Arc<runtrol_runtime_protocol::ProviderList>>,
    usage: &watch::Sender<Arc<runtrol_runtime_protocol::ProviderUsageList>>,
) {
    let ids: Vec<ProviderId> = composed
        .registry
        .all()
        .filter(|provider| provider.is_usable())
        .map(runtrol_core::registry::Provider::id)
        .collect();
    let mut changed = false;
    for id in ids {
        let Some(report) = ask(composed, id).await else {
            continue;
        };
        let now = WallMs::now();
        let mut reports = composed.account_reports.lock().await;
        let same = reports.get(id).is_some_and(|known| known.report == report);
        reports.record(id, report, now);
        changed |= !same;
    }
    if !changed {
        return;
    }
    // The provider descriptors carry the status and the usage list carries the probed windows; both
    // projections read the reports on their own, so publishing is asking each to look again.
    crate::runtime_inventory::invalidate_provider_inventory(composed).await;
    let next = Arc::new(crate::runtime_inventory::providers(composed));
    providers.send_replace(next);
    usage.send_modify(|current| {
        let merged = crate::runtime_inventory::merge_probed_usage(current.as_ref(), composed);
        *current = Arc::new(merged);
    });
}

/// One service's answer, or nothing when it could not even be prepared, which is an installation
/// problem the inventory already names.
async fn ask(composed: &Arc<Composed>, id: ProviderId) -> Option<AccountReport> {
    let Ok(driver) = crate::provider_prepare::driver(composed, id).await else {
        return None;
    };
    match tokio::time::timeout(ACCOUNT_PROBE_DEADLINE, driver.account()).await {
        Ok(Ok(report)) => Some(report),
        Ok(Err(error)) => Some(AccountReport::unpublished(&format!(
            "the service did not answer its status surface: {error}"
        ))),
        Err(_) => Some(AccountReport::unpublished(
            "the service did not answer its status surface within the deadline",
        )),
    }
}
