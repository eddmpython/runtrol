//! Asking each installed coding service where the operator's account stands, on the service's own
//! status surface, and remembering the answer for the provider and usage projections.
//!
//! Starts on the first account subscriber or provider activity signal, then runs on a slow clock. Nothing here
//! reads a credential file or a transcript: a driver either has a published status surface (`claude auth status
//! --json`, Codex `account/read`) or says it has none, and the projections repeat exactly that.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use runtrol_provider::{AccountReport, AccountStatus, ProviderId, WallMs};
use tokio::sync::{Notify, watch};

use crate::Composed;

/// How long one service may take to answer.
///
/// Generous, because what one service does to answer is not what another does. Measured 2026-08-26 on this
/// machine: two answer in about three seconds each over a protocol they already speak, and the third takes
/// about fourteen because it opens its own headless channel, asks its vendor where the account stands, and
/// shuts down again. A tighter bound would turn ordinary slow reads into failures. Failed reads remain
/// distinct from an unsupported account surface, and a slow provider never delays another provider.
const ACCOUNT_PROBE_DEADLINE: Duration = Duration::from_mins(1);
/// How often every service is asked with nothing else prompting it: the backstop, not the driver.
///
/// The rounds that matter are the ones a service's own terminal prompts, so this is the slow sweep that
/// catches what happened somewhere else: a turn taken on the operator's phone, a plan changed in a browser,
/// a limit that reset while nothing was open here.
const ROUND_INTERVAL: Duration = Duration::from_mins(10);
/// How long a service's terminal has to be quiet before its answer is worth asking for again.
///
/// Output becoming quiet is only a cue to ask the provider for current account data. It never proves a
/// model turn ended or authorizes stopping the process. Coalescing avoids querying between adjacent TUI frames.
const TURN_QUIET: Duration = Duration::from_secs(1);
/// The least time between two questions to one service.
///
/// A question is not free: measured 2026-08-27, asking one of these three costs a child process that peaks
/// near 470 MiB for eight seconds, because answering means that CLI opening its own channel to its vendor.
/// Short turns in a row would otherwise pay that every few seconds. Per service rather than overall, so a
/// slow answer from one never delays another.
const PROCESS_SERVICE_FLOOR: Duration = Duration::from_secs(30);
/// The least time between two questions over a declared structured account protocol.
///
/// These drivers reuse or briefly open their machine channel and measured in well under the process-backed
/// account reader. The distinction comes from the provider manifest, never a provider name, so a new driver
/// gets the right lane by declaring the surface it actually owns.
const PROTOCOL_SERVICE_FLOOR: Duration = Duration::from_secs(5);
/// How soon a question that did not come back is asked again.
///
/// A read that failed is the one absence that is runtrol's own, so it is not left to the slow sweep: the
/// row says "Usage unreadable" and the loop tries again a minute later rather than in ten. Long enough that
/// a service which is down is not hammered, short enough that a blip heals itself while somebody watches.
const RETRY_AFTER: Duration = Duration::from_mins(1);
/// How often the loop looks at the terminals.
///
/// Cheap on purpose: it reads one atomic per open terminal and starts nothing unless one of them has just
/// gone quiet. When nothing is open it does not run at all, so an idle daemon keeps the footprint its
/// budget contract fixes.
const WATCH_TICK: Duration = Duration::from_millis(500);
/// How long a wake waits before its round, so a burst of session events becomes one round.
const WAKE_SETTLE: Duration = Duration::from_millis(250);

/// A bounded, coalescing request for fresh account state.
///
/// A notification alone loses which provider moved and turns one finished conversation into a probe of every
/// installed service. This retains at most one bit per provider plus one all-services bit, so a burst from any
/// number of windows has memory bounded by the provider registry rather than the event rate.
#[derive(Debug, Default)]
pub(crate) struct AccountProbeWake {
    pending: tokio::sync::Mutex<ProbeRequest>,
    notify: Notify,
}

#[derive(Debug, Default)]
struct ProbeRequest {
    all: bool,
    providers: BTreeSet<ProviderId>,
}

impl ProbeRequest {
    fn is_empty(&self) -> bool {
        !self.all && self.providers.is_empty()
    }

    fn merge(&mut self, mut other: Self) {
        self.all |= other.all;
        self.providers.append(&mut other.providers);
    }
}

impl AccountProbeWake {
    /// Ask for every usable provider. Used when a usage surface first becomes visible.
    pub(crate) async fn all(&self) {
        self.pending.lock().await.all = true;
        self.notify.notify_one();
    }

    /// Ask only the provider whose activity changed.
    pub(crate) async fn provider(&self, provider: ProviderId) {
        // ok: duplicate activity signals deliberately collapse into one provider identity.
        self.pending.lock().await.providers.insert(provider);
        self.notify.notify_one();
    }

    async fn wait(&self) -> ProbeRequest {
        loop {
            self.notify.notified().await;
            let pending = self.take().await;
            if !pending.is_empty() {
                return pending;
            }
        }
    }

    async fn take(&self) -> ProbeRequest {
        std::mem::take(&mut *self.pending.lock().await)
    }
}

/// One service's latest report and when it arrived.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Reported {
    pub(crate) report: AccountReport,
    pub(crate) at: WallMs,
}

impl Reported {
    fn gauge(&self, provider: ProviderId) -> Option<runtrol_core::ProviderGauge> {
        let limit = self.report.as_rate_limit()?;
        Some(runtrol_core::ProviderGauge {
            provider,
            reached: limit.reached,
            windows: limit.windows,
            cost: None,
            tokens_today: self.report.tokens_today,
            at: self.at,
        })
    }
}

type ProbeAnswer = (ProviderId, Result<AccountReport, String>, WallMs);

#[derive(Default)]
struct AccountProbes {
    tasks: tokio::task::JoinSet<ProbeAnswer>,
    owners: BTreeMap<tokio::task::Id, ProviderId>,
}

impl AccountProbes {
    fn spawn(
        &mut self,
        provider: ProviderId,
        read: impl Future<Output = Result<AccountReport, String>> + Send + 'static,
    ) -> bool {
        if self.contains(provider) {
            return false;
        }
        let task = self
            .tasks
            .spawn(async move { (provider, read.await, WallMs::now()) });
        self.owners.insert(task.id(), provider);
        true
    }

    fn contains(&self, provider: ProviderId) -> bool {
        self.owners.values().any(|owner| *owner == provider)
    }

    fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    #[expect(
        clippy::print_stderr,
        reason = "a lost task-owner invariant has no provider or request to answer; stderr is the daemon's operational failure channel"
    )]
    async fn next(&mut self) -> Option<ProbeAnswer> {
        loop {
            match self.tasks.join_next_with_id().await? {
                Ok((task, answer)) => {
                    self.owners.remove(&task);
                    return Some(answer);
                }
                Err(error) => {
                    let Some(provider) = self.owners.remove(&error.id()) else {
                        // A broken task-owner invariant cannot be attributed to an arbitrary provider.
                        // Report it and keep draining independent answers rather than stopping the supervisor.
                        eprintln!("account task lost its registered owner: {error}");
                        continue;
                    };
                    return Some((
                        provider,
                        Err(format!("the account read task did not complete: {error}")),
                        WallMs::now(),
                    ));
                }
            }
        }
    }
}

/// Every service's latest report.
#[derive(Debug, Default)]
pub(crate) struct AccountReports {
    latest: BTreeMap<ProviderId, Reported>,
    failed: BTreeMap<ProviderId, FailedRead>,
    // Only a partial read needs this: its new identity fact must not relabel older usage as freshly read.
    retained_gauges: BTreeMap<ProviderId, runtrol_core::ProviderGauge>,
}

#[derive(Debug)]
struct FailedRead {
    why: String,
    at: WallMs,
}

impl AccountReports {
    /// Remember one answer. The newest always wins: the report answers "now".
    pub(crate) fn record(&mut self, provider: ProviderId, report: AccountReport, at: WallMs) {
        if report.limits.is_none()
            && matches!(report.status, AccountStatus::SignedIn)
            && report
                .limits_absent
                .as_ref()
                .is_some_and(runtrol_provider::LimitsAbsent::is_worth_retrying)
        {
            if let Some(gauge) = self
                .latest
                .get(&provider)
                .and_then(|previous| previous.gauge(provider))
            {
                self.retained_gauges.insert(provider, gauge);
            }
        } else {
            self.retained_gauges.remove(&provider);
        }
        self.latest.insert(provider, Reported { report, at });
        self.failed.remove(&provider);
    }

    fn record_failure(&mut self, provider: ProviderId, why: String, at: WallMs) {
        self.failed.insert(provider, FailedRead { why, at });
    }

    /// Store the actual completion time, including an unchanged successful answer.
    fn record_answer(&mut self, answer: ProbeAnswer) -> bool {
        let (provider, result, at) = answer;
        match result {
            Ok(report) => {
                let retry = report
                    .limits_absent
                    .as_ref()
                    .is_some_and(runtrol_provider::LimitsAbsent::is_worth_retrying);
                self.record(provider, report, at);
                retry
            }
            Err(why) => {
                self.record_failure(provider, why, at);
                true
            }
        }
    }

    pub(crate) fn get(&self, provider: ProviderId) -> Option<&Reported> {
        self.latest.get(&provider)
    }

    /// A successful no-usage verdict waits for a real activity, account action, or installation change.
    fn needs_sweep(&self, provider: ProviderId) -> bool {
        self.get(provider).is_none_or(|reported| {
            reported.report.limits.is_some()
                || reported
                    .report
                    .limits_absent
                    .as_ref()
                    .is_some_and(runtrol_provider::LimitsAbsent::is_worth_retrying)
        })
    }

    /// A successful absence retires older gauges already merged into a watch value. A failed read retains
    /// evidence, but an unsupported surface or unmetered account must not keep publishing old numbers.
    pub(crate) fn usage_absent(&self) -> impl Iterator<Item = (ProviderId, WallMs)> + '_ {
        self.latest.iter().filter_map(|(id, report)| {
            (report.report.limits.is_none()
                && !report
                    .report
                    .limits_absent
                    .as_ref()
                    .is_some_and(runtrol_provider::LimitsAbsent::is_worth_retrying))
            .then_some((*id, report.at))
        })
    }

    /// The public shape of one report, for the provider descriptor.
    pub(crate) fn descriptor(
        &self,
        provider: ProviderId,
    ) -> Option<runtrol_runtime_protocol::ProviderAccount> {
        if let Some(failed) = self.failed.get(&provider) {
            return Some(runtrol_runtime_protocol::ProviderAccount {
                status: runtrol_runtime_protocol::ProviderAccountStatus::Unread,
                plan: None,
                method: None,
                why: Some(failed.why.clone()),
                limits_absent: None,
                checked_at_ms: failed.at.as_millis(),
            });
        }
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
            limits_absent: reported.report.limits_absent.as_ref().map(|absent| {
                runtrol_runtime_protocol::ProviderLimitsAbsent {
                    kind: if absent.is_worth_retrying() {
                        runtrol_runtime_protocol::ProviderLimitsAbsentKind::Unread
                    } else {
                        runtrol_runtime_protocol::ProviderLimitsAbsentKind::Unmetered
                    },
                    why: absent.why().to_owned(),
                }
            }),
            checked_at_ms: reported.at.as_millis(),
        })
    }

    /// Every service whose report carried limit windows, as gauges the usage list can merge.
    pub(crate) fn probed_gauges(&self) -> Vec<runtrol_core::ProviderGauge> {
        self.latest
            .iter()
            .filter_map(|(provider, reported)| {
                reported
                    .gauge(*provider)
                    .or_else(|| self.retained_gauges.get(provider).cloned())
            })
            .collect()
    }
}

/// Ask each service where its account stands, when that service's answer has changed.
///
/// Four things prompt a question, in the order they matter.
///
/// **The first subscriber.** Provider usage and watch requests wake every service. With no subscriber and no
/// provider activity there is nobody to consume an account report, so daemon startup opens no account process.
///
/// **A conversation went quiet.** A conversation held as its CLI's own terminal publishes no turn boundary,
/// so the boundary is the CLI writing and then stopping. That is the moment the number moved, and asking
/// then is what makes the strip live rather than a thing that catches up on a clock.
///
/// **Something opened or closed.** A conversation starting or ending is a state change worth a question,
/// and it is what fills the strip in the first seconds after a window opens.
///
/// **The slow sweep.** Everything else that can move an account: a turn taken on the operator's phone, a
/// plan changed in a browser, a window that reset while nothing was open here.
///
/// What it deliberately does not do is ask on a fast clock. Measured 2026-08-27, one question to one of
/// these services costs a child process peaking near 470 MiB for eight seconds, so a ninety-second sweep of
/// three services spent that on two services nobody had touched. Asking only the service that moved is both
/// the cheaper answer and the more current one.
pub(crate) async fn supervise(
    composed: Arc<Composed>,
    providers: watch::Sender<Arc<runtrol_runtime_protocol::ProviderList>>,
    usage: watch::Sender<Arc<runtrol_runtime_protocol::ProviderUsageList>>,
) {
    let first = composed.account_probe_wake.wait().await;
    let mut first = settled_request(&composed.account_probe_wake, first).await;
    first.merge(composed.account_probe_wake.take().await);
    let mut schedule = ProbeSchedule {
        asked: BTreeMap::new(),
        unread: BTreeMap::new(),
        pending: requested(&composed, &first).into_iter().collect(),
        swept_at: WallMs::now(),
        wake_ready: None,
    };
    let mut probes = AccountProbes::default();
    loop {
        schedule.start_due(&composed, &mut probes).await;
        let wait = schedule.next_check(
            composed
                .open_terminals
                .load(std::sync::atomic::Ordering::Acquire)
                > 0,
            &probes,
            WallMs::now(),
            |provider| service_floor(&composed, provider),
        );
        tokio::select! {
            answer = probes.next(), if !probes.is_empty() => {
                if let Some(answer) = answer {
                    let (id, _, at) = &answer;
                    let (id, at) = (*id, *at);
                    let refresh_inventory = probes.is_empty() || providers.borrow().providers.is_empty();
                    let retry = publish_answer(&composed, &providers, &usage, answer, refresh_inventory).await;
                    schedule.completed(id, retry, at);
                    // Evicting hot pages taxes input. Trim only after all reads and terminals are idle.
                    if probes.is_empty() && composed.open_terminals.load(std::sync::atomic::Ordering::Acquire) == 0 {
                        runtrol_childproc::footprint::release_unused_memory();
                    }
                }
            }
            request = composed.account_probe_wake.wait() => {
                schedule.pending.extend(requested(&composed, &request));
                schedule.wake_ready.get_or_insert(tokio::time::Instant::now() + WAKE_SETTLE);
            }
            () = tokio::time::sleep(wait) => {}
        }
    }
}

struct ProbeSchedule {
    asked: BTreeMap<ProviderId, WallMs>,
    // Retry is measured from the failed completion, not the start of a potentially minute-long read.
    unread: BTreeMap<ProviderId, WallMs>,
    pending: BTreeSet<ProviderId>,
    swept_at: WallMs,
    wake_ready: Option<tokio::time::Instant>,
}

impl ProbeSchedule {
    async fn start_due(&mut self, composed: &Arc<Composed>, probes: &mut AccountProbes) {
        if self
            .wake_ready
            .is_some_and(|at| at <= tokio::time::Instant::now())
        {
            self.wake_ready = None;
        }
        let now = WallMs::now();
        let due = due_now(composed, &self.asked, self.swept_at, now).await;
        if self.swept_at.millis_until(now).unwrap_or(0) >= millis(ROUND_INTERVAL) {
            // An in-flight service already owns this sweep. It must not leave a zero-duration idle loop.
            self.swept_at = now;
        }
        // A read already in flight owns a sweep/retry request. An explicit wake remains pending instead.
        self.pending
            .extend(due.into_iter().filter(|id| !probes.contains(*id)));
        self.pending
            .extend(self.unread.iter().filter_map(|(id, at)| {
                (!probes.contains(*id) && at.millis_until(now).unwrap_or(0) >= millis(RETRY_AFTER))
                    .then_some(*id)
            }));
        if self.wake_ready.is_some() {
            return;
        }
        // Every trigger shares the same cost floor, including a sweep just after a real activity signal.
        let due = take_due(
            &mut self.pending,
            &self.asked,
            now,
            |provider| service_floor(composed, provider),
            |provider| !probes.contains(provider),
        );
        for id in due {
            let reading = Arc::clone(composed);
            if probes.spawn(id, async move { ask(&reading, id).await }) {
                self.asked.insert(id, now);
            }
        }
    }

    fn completed(&mut self, id: ProviderId, retry: bool, at: WallMs) {
        if retry {
            self.unread.insert(id, at);
        } else {
            self.unread.remove(&id);
        }
    }

    /// With no terminal or due request, sleep directly to a retry or the slow maintenance sweep.
    fn next_check(
        &self,
        hot: bool,
        probes: &AccountProbes,
        now: WallMs,
        floor: impl Fn(ProviderId) -> Duration,
    ) -> Duration {
        let mut next = remaining(self.swept_at, ROUND_INTERVAL, now);
        if hot {
            next = next.min(WATCH_TICK);
        }
        for (provider, at) in &self.unread {
            if !probes.contains(*provider) {
                next = next.min(remaining(*at, RETRY_AFTER, now));
            }
        }
        if let Some(at) = self.wake_ready {
            next = next.min(at.saturating_duration_since(tokio::time::Instant::now()));
        } else {
            for provider in &self.pending {
                if !probes.contains(*provider) {
                    next = next.min(
                        self.asked
                            .get(provider)
                            .map_or(Duration::ZERO, |at| remaining(*at, floor(*provider), now)),
                    );
                }
            }
        }
        next
    }
}

fn remaining(since: WallMs, interval: Duration, now: WallMs) -> Duration {
    interval.saturating_sub(Duration::from_millis(since.millis_until(now).unwrap_or(0)))
}

async fn settled_request(wake: &AccountProbeWake, mut request: ProbeRequest) -> ProbeRequest {
    tokio::time::sleep(WAKE_SETTLE).await;
    request.merge(wake.take().await);
    request
}

/// Every service this build can actually ask.
fn usable(composed: &Composed) -> Vec<ProviderId> {
    composed
        .registry
        .all()
        .filter(|provider| provider.is_usable())
        .map(runtrol_core::registry::Provider::id)
        .collect()
}

/// Usable providers named by one coalesced wake.
fn requested(composed: &Composed, request: &ProbeRequest) -> Vec<ProviderId> {
    let requested = if request.all {
        usable(composed)
    } else {
        request.providers.iter().copied().collect()
    };
    requested
        .into_iter()
        .filter(|id| {
            composed
                .registry
                .get(*id)
                .is_some_and(runtrol_core::registry::Provider::is_usable)
        })
        .collect()
}

/// Remove and return pending providers whose measured cost floor has passed.
///
/// A request inside its floor remains in the bounded set. Dropping it made a second external turn disappear
/// until the ten-minute sweep because no hosted terminal clock existed to rediscover that quiet edge.
fn take_due(
    pending: &mut BTreeSet<ProviderId>,
    asked: &BTreeMap<ProviderId, WallMs>,
    now: WallMs,
    floor: impl Fn(ProviderId) -> Duration,
    available: impl Fn(ProviderId) -> bool,
) -> Vec<ProviderId> {
    let due: Vec<ProviderId> = pending
        .iter()
        .copied()
        .filter(|id| available(*id))
        .filter(|id| {
            asked
                .get(id)
                .is_none_or(|at| at.millis_until(now).unwrap_or(0) >= millis(floor(*id)))
        })
        .collect();
    for id in &due {
        pending.remove(id);
    }
    due
}

/// The cost floor declared by the account transport rather than the provider's identity.
fn service_floor(composed: &Composed, provider: ProviderId) -> Duration {
    let protocol = composed
        .registry
        .get(provider)
        .and_then(|provider| provider.manifest.account.as_ref())
        .and_then(|account| account.protocol.as_ref());
    if protocol.is_some() {
        PROTOCOL_SERVICE_FLOOR
    } else {
        PROCESS_SERVICE_FLOOR
    }
}

/// One duration as whole milliseconds a wall-clock difference can be compared against.
///
/// These are constants of a few seconds, so the conversion cannot lose anything; it is written as a
/// checked step anyway because a cast that silently truncates is how a bound of thirty seconds becomes a
/// bound of no seconds.
fn millis(span: Duration) -> u64 {
    u64::try_from(span.as_millis()).unwrap_or(u64::MAX)
}

/// Whether one service is worth asking again, from when its CLI last wrote and when it was last asked.
///
/// The whole decision, kept apart from the table it is made over so it can be stated as cases.
fn is_due(wrote: Option<WallMs>, asked: Option<WallMs>, now: WallMs, floor: Duration) -> bool {
    // Wrote something, then stopped: that is this surface's turn boundary. A terminal that has never
    // written has nothing to have finished.
    let Some(wrote) = wrote else { return false };
    if wrote.millis_until(now).unwrap_or(0) < millis(TURN_QUIET) {
        return false;
    }
    match asked {
        // Nothing written since the last answer, so the last answer is still the answer.
        Some(asked) if asked >= wrote => false,
        Some(asked) => asked.millis_until(now).unwrap_or(0) >= millis(floor),
        None => true,
    }
}

/// Which services are worth asking right now, and nothing when none are.
///
/// The old loop asked every service on one clock. That was wrong twice over: it asked about a service
/// nobody had touched in an hour, and it did not ask about the one somebody had just finished a turn with
/// until the clock came round. This asks the service whose CLI just stopped writing, which is the moment
/// its answer changed, and asks the rest only on the slow sweep.
async fn due_now(
    composed: &Composed,
    asked: &BTreeMap<ProviderId, WallMs>,
    swept_at: WallMs,
    now: WallMs,
) -> Vec<ProviderId> {
    let usable = || -> Vec<ProviderId> {
        composed
            .registry
            .all()
            .filter(|provider| provider.is_usable())
            .map(runtrol_core::registry::Provider::id)
            .collect()
    };
    if swept_at.millis_until(now).unwrap_or(0) >= millis(ROUND_INTERVAL) {
        let reports = composed.account_reports.lock().await;
        return usable()
            .into_iter()
            .filter(|id| reports.needs_sweep(*id))
            .collect();
    }
    if composed
        .open_terminals
        .load(std::sync::atomic::Ordering::Acquire)
        == 0
    {
        return Vec::new();
    }
    let wrote = {
        let terminals = composed.terminals.lock().await;
        terminals.wrote_at_by_provider()
    };
    let usable = usable();
    wrote
        .into_iter()
        .filter(|(provider, _)| usable.contains(provider))
        .filter(|(provider, wrote)| {
            is_due(
                *wrote,
                asked.get(provider).copied(),
                now,
                service_floor(composed, *provider),
            )
        })
        .map(|(provider, _)| provider)
        .collect()
}

/// Publish one completed provider read without waiting for unrelated providers.
#[expect(
    clippy::print_stderr,
    reason = "a detached provider inventory rebuild has no request to answer; stderr is the daemon's operational failure channel"
)]
async fn publish_answer(
    composed: &Arc<Composed>,
    providers: &watch::Sender<Arc<runtrol_runtime_protocol::ProviderList>>,
    usage: &watch::Sender<Arc<runtrol_runtime_protocol::ProviderUsageList>>,
    answer: ProbeAnswer,
    refresh_inventory: bool,
) -> bool {
    let provider = answer.0;
    let (retry, account) = {
        let mut reports = composed.account_reports.lock().await;
        let retry = reports.record_answer(answer);
        (retry, reports.descriptor(provider))
    };
    publish_account(providers, provider, account);
    usage.send_modify(|current| {
        *current = Arc::new(crate::runtime_inventory::merge_probed_usage(
            current.as_ref(),
            composed,
        ));
    });
    // Account state is already visible. Installation metadata can wait for the end of this group of reads;
    // asking the filesystem once per completion would turn independent results into repeated PATH scans.
    crate::runtime_inventory::invalidate_provider_inventory(composed).await;
    if !refresh_inventory {
        return retry;
    }
    match crate::runtime_inventory::providers_in_background(Arc::clone(composed)).await {
        Ok(Some(next)) => {
            let next = Arc::new(next);
            providers.send_if_modified(|current| {
                if current.as_ref() == next.as_ref() {
                    return false;
                }
                *current = next;
                true
            });
        }
        Ok(None) => {}
        Err(error) => eprintln!("{error}"),
    }
    retry
}

fn publish_account(
    providers: &watch::Sender<Arc<runtrol_runtime_protocol::ProviderList>>,
    provider: ProviderId,
    account: Option<runtrol_runtime_protocol::ProviderAccount>,
) {
    providers.send_if_modified(|current| {
        let Some(entry) = current
            .providers
            .iter()
            .find(|entry| entry.provider_id.as_str() == provider.as_str())
        else {
            // A usage-only subscriber may precede the first inventory. The initial rebuild fills that list.
            return false;
        };
        if entry.account == account {
            return false;
        }
        if let Some(entry) = Arc::make_mut(current)
            .providers
            .iter_mut()
            .find(|entry| entry.provider_id.as_str() == provider.as_str())
        {
            entry.account = account;
            true
        } else {
            false
        }
    });
}

/// One service's answer, or an explicit failed read including driver preparation failure.
async fn ask(composed: &Arc<Composed>, id: ProviderId) -> Result<AccountReport, String> {
    // Keep the exact resolved invocation so completion needs only its metadata, never another PATH search.
    let prepared = crate::provider_prepare::prepared_terminal_driver(composed, id)
        .await
        .map_err(|error| error.message().to_owned())?;
    let program = prepared
        .terminal_program
        .ok_or_else(|| "account preparation did not retain its executable".to_owned())?;
    let answer = read_account(prepared.driver.account()).await;
    let current = runtrol_core::probe::inspect_program(&program)
        .await
        .map_err(|error| error.to_string())?;
    if Some(&current) != prepared.program_facts.as_deref() {
        composed.account_probe_wake.provider(id).await;
        return Err(
            "the provider CLI changed while reading usage; checking the current installation"
                .to_owned(),
        );
    }
    answer
}

async fn read_account(
    read: impl Future<Output = Result<AccountReport, runtrol_provider::ProviderError>>,
) -> Result<AccountReport, String> {
    read_account_within(read, ACCOUNT_PROBE_DEADLINE).await
}

async fn read_account_within(
    read: impl Future<Output = Result<AccountReport, runtrol_provider::ProviderError>>,
    deadline: Duration,
) -> Result<AccountReport, String> {
    match tokio::time::timeout(deadline, read).await {
        Ok(Ok(report)) => Ok(report),
        Ok(Err(error)) => Err(format!(
            "the service did not answer its status surface: {error}"
        )),
        Err(_) => {
            Err("the service did not answer its status surface within the deadline".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_limit_failures_keep_only_the_previous_gauge_and_its_original_time() {
        let id = ProviderId::parse("partial").expect("provider");
        let mut reports = AccountReports::default();
        let mut report = AccountReport::unpublished("fixture");
        report.status = AccountStatus::SignedIn;
        report.limits = Some(runtrol_provider::AccountLimits::new(Vec::new(), false));
        report.tokens_today = Some(42);
        reports.record(id, report.clone(), WallMs::from_millis(10));
        report.limits = None;
        report.tokens_today = None;
        report.limits_absent = Some(runtrol_provider::LimitsAbsent::Unread {
            why: "limit timeout".into(),
        });
        for at in [20, 30] {
            assert!(reports.record_answer((id, Ok(report.clone()), WallMs::from_millis(at))));
            let gauges = reports.probed_gauges();
            assert_eq!(gauges.len(), 1);
            let gauge = gauges.first().expect("one retained gauge");
            assert_eq!(gauge.tokens_today, Some(42));
            assert_eq!(gauge.at, WallMs::from_millis(10));
            assert_eq!(
                reports
                    .descriptor(id)
                    .expect("fresh identity")
                    .checked_at_ms,
                at
            );
        }
        report.limits_absent = Some(runtrol_provider::LimitsAbsent::Unmetered {
            why: "account changed to team metering".into(),
        });
        assert!(!reports.record_answer((id, Ok(report), WallMs::from_millis(40))));
        assert!(
            reports.probed_gauges().is_empty(),
            "a real replacement verdict clears the retained gauge"
        );
        assert!(reports.retained_gauges.is_empty());
        assert!(
            !reports.needs_sweep(id),
            "a confirmed absence is not polled"
        );
    }

    #[test]
    fn a_completed_account_is_pushed_without_an_installation_scan() {
        let id = ProviderId::parse("visible").expect("provider");
        let initial = serde_json::from_value(serde_json::json!({
            "providers": [{"providerId": "visible", "displayName": "Visible", "installation": {"state": "usable"}}]
        })).expect("minimal provider inventory");
        let (sender, mut updates) = watch::channel(Arc::new(initial));
        let mut reports = AccountReports::default();
        reports.record_failure(id, "account timeout".to_owned(), WallMs::from_millis(20));
        publish_account(&sender, id, reports.descriptor(id));
        assert!(updates.has_changed().expect("watch is open"));
        assert_eq!(
            updates
                .borrow_and_update()
                .providers
                .first()
                .expect("one provider")
                .account
                .as_ref()
                .expect("unread visible")
                .status,
            runtrol_runtime_protocol::ProviderAccountStatus::Unread
        );
        reports.record(
            id,
            AccountReport::unpublished("no declared surface"),
            WallMs::from_millis(30),
        );
        publish_account(&sender, id, reports.descriptor(id));
        assert_eq!(
            updates
                .borrow_and_update()
                .providers
                .first()
                .expect("one provider")
                .account
                .as_ref()
                .expect("recovery visible")
                .checked_at_ms,
            30
        );
    }

    #[tokio::test]
    async fn completed_account_is_delivered_while_another_provider_is_still_waiting() {
        let fast = ProviderId::parse("fast").expect("provider");
        let slow = ProviderId::parse("slow").expect("provider");
        let (ready, completed) = tokio::sync::oneshot::channel();
        let (_release, blocked) = tokio::sync::oneshot::channel::<()>();
        let mut probes = AccountProbes::default();
        probes.spawn(slow, async move {
            blocked.await.expect("test retains the release owner");
            Ok(AccountReport::unpublished("no account surface"))
        });
        probes.spawn(fast, async move {
            ready.send(()).expect("test is waiting");
            Ok(AccountReport::unpublished("no account surface"))
        });
        // The current-thread task finishes without yielding after this signal. No elapsed-time assertion.
        completed.await.expect("fast provider completed");
        {
            let mut next = std::pin::pin!(probes.next());
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(
                matches!(next.as_mut().poll(&mut context), std::task::Poll::Ready(Some((id, Ok(_), _))) if id == fast)
            );
        }
        assert!(probes.contains(slow));
        assert!(
            !probes.spawn(slow, std::future::pending()),
            "one provider owns only one read"
        );
        assert!(probes.spawn(fast, async {
            Ok(AccountReport::unpublished("second independent read"))
        }));
        assert_eq!(
            probes
                .next()
                .await
                .expect("fast provider completes again")
                .0,
            fast
        );
        assert!(
            probes.contains(slow),
            "the first slow read has still not been released"
        );
    }

    #[tokio::test]
    async fn a_failed_task_remains_an_owned_unread_result_and_can_be_asked_again() {
        let id = ProviderId::parse("panicking").expect("provider");
        let mut probes = AccountProbes::default();
        assert!(probes.spawn(id, async { panic!("injected account task failure") }));
        let answer = probes
            .next()
            .await
            .expect("the failure keeps its provider identity");
        assert_eq!(answer.0, id);
        assert!(answer.1.is_err());
        assert!(probes.is_empty());
        assert!(!probes.contains(id));
        let mut reports = AccountReports::default();
        assert!(reports.record_answer(answer));
        assert_eq!(
            reports.descriptor(id).expect("failed read").status,
            runtrol_runtime_protocol::ProviderAccountStatus::Unread
        );
        assert!(probes.spawn(id, async { Ok(AccountReport::unpublished("recovered")) }));
        assert!(!reports.record_answer(probes.next().await.expect("recovered")));
    }

    #[tokio::test]
    async fn retries_wait_from_completion_but_a_real_signal_uses_only_its_service_floor() {
        let id = ProviderId::parse("retry").expect("provider");
        let started = WallMs::from_millis(10_000);
        let finished = WallMs::from_millis(70_000);
        let mut schedule = ProbeSchedule {
            asked: BTreeMap::from([(id, started)]),
            unread: BTreeMap::new(),
            pending: BTreeSet::new(),
            swept_at: started,
            wake_ready: None,
        };
        schedule.completed(id, true, finished);
        let mut probes = AccountProbes::default();
        assert_eq!(
            schedule.next_check(false, &probes, finished, |_| PROCESS_SERVICE_FLOOR),
            RETRY_AFTER
        );
        assert_eq!(
            schedule.next_check(false, &probes, WallMs::from_millis(130_000), |_| {
                PROCESS_SERVICE_FLOOR
            }),
            Duration::ZERO
        );
        schedule.pending.insert(id);
        assert_eq!(
            schedule.next_check(false, &probes, finished, |_| PROCESS_SERVICE_FLOOR),
            Duration::ZERO
        );
        assert!(probes.spawn(id, std::future::pending()));
        assert!(
            take_due(
                &mut schedule.pending,
                &schedule.asked,
                finished,
                |_| PROCESS_SERVICE_FLOOR,
                |provider| !probes.contains(provider)
            )
            .is_empty()
        );
        assert!(
            schedule.pending.contains(&id),
            "a wake during a read survives until that read finishes"
        );
        assert_eq!(
            schedule.next_check(false, &probes, finished, |_| PROCESS_SERVICE_FLOOR),
            Duration::from_mins(9),
            "an in-flight retry or wake cannot busy-spin"
        );
        schedule.completed(id, false, finished);
        assert!(
            schedule.unread.is_empty(),
            "a real answer clears automatic retry"
        );
    }

    #[test]
    fn an_unchanged_success_keeps_its_actual_completion_time_and_clears_failure() {
        let id = ProviderId::parse("same").expect("provider");
        let report = AccountReport::unpublished("declared unsupported surface");
        let mut reports = AccountReports::default();
        assert!(!reports.record_answer((id, Ok(report.clone()), WallMs::from_millis(10))));
        assert!(reports.record_answer((
            id,
            Err("temporary read failure".to_owned()),
            WallMs::from_millis(20)
        )));
        assert!(!reports.record_answer((id, Ok(report), WallMs::from_millis(30))));
        let descriptor = reports.descriptor(id).expect("latest completed answer");
        assert_eq!(descriptor.checked_at_ms, 30);
        assert!(
            !reports.needs_sweep(id),
            "an unsupported surface waits for a real change"
        );
        assert_eq!(
            descriptor.status,
            runtrol_runtime_protocol::ProviderAccountStatus::Unpublished
        );
    }

    #[tokio::test]
    async fn failed_account_read_is_not_an_unsupported_surface() {
        let provider = ProviderId::parse("broken").expect("provider");
        let answer = read_account(async move {
            Err(runtrol_provider::ProviderError::Protocol {
                provider,
                doing: "reading account",
                detail: "incomplete structured response".to_owned(),
            })
        })
        .await;
        assert!(
            answer.is_err(),
            "a failed read is not the provider reporting no surface"
        );
    }

    #[tokio::test]
    async fn expired_account_read_is_not_an_unsupported_surface() {
        let answer = read_account_within(std::future::pending(), Duration::ZERO).await;
        assert!(
            answer.is_err(),
            "a deadline provides no provider account verdict"
        );
    }

    #[test]
    fn failed_reads_preserve_the_last_provider_gauge_and_original_time() {
        let provider = ProviderId::parse("metered").expect("provider");
        let at = WallMs::from_millis(10_000);
        let failed_at = WallMs::from_millis(20_000);
        let mut report = AccountReport::unpublished("fixture");
        report.status = AccountStatus::SignedIn;
        report.limits = Some(runtrol_provider::AccountLimits::new(Vec::new(), false));
        report.tokens_today = Some(42);
        let mut reports = AccountReports::default();
        reports.record(provider, report, at);
        reports.record_failure(provider, "status timeout".to_owned(), failed_at);
        let descriptor = reports.descriptor(provider).expect("failure is visible");
        assert_eq!(
            descriptor.status,
            runtrol_runtime_protocol::ProviderAccountStatus::Unread
        );
        assert_eq!(descriptor.checked_at_ms, failed_at.as_millis());
        assert_eq!(reports.get(provider).expect("last success survives").at, at);
        let gauges = reports.probed_gauges();
        assert_eq!(gauges.len(), 1);
        let gauge = gauges.first().expect("one retained gauge");
        assert_eq!(gauge.at, at);
        assert_eq!(gauge.tokens_today, Some(42));
        let mut signed_out = AccountReport::unpublished("fixture");
        signed_out.status = AccountStatus::SignedOut;
        reports.record(provider, signed_out, WallMs::from_millis(30_000));
        assert_eq!(
            reports.descriptor(provider).expect("recovered").status,
            runtrol_runtime_protocol::ProviderAccountStatus::SignedOut
        );
        assert!(
            reports.probed_gauges().is_empty(),
            "a provider's new signed-out answer replaces its old probe gauge"
        );
    }

    #[test]
    fn a_first_failed_read_has_no_invented_account_or_usage() {
        let provider = ProviderId::parse("unknown").expect("provider");
        let mut reports = AccountReports::default();
        reports.record_failure(
            provider,
            "unread response".to_owned(),
            WallMs::from_millis(20),
        );
        let descriptor = reports
            .descriptor(provider)
            .expect("failed read is visible");
        assert_eq!(
            descriptor.status,
            runtrol_runtime_protocol::ProviderAccountStatus::Unread
        );
        assert!(descriptor.plan.is_none());
        assert!(reports.get(provider).is_none());
        assert!(
            reports.needs_sweep(provider),
            "a failure has not established an absent surface"
        );
        assert!(reports.probed_gauges().is_empty());
        reports.record(
            provider,
            AccountReport::unpublished("no declared account surface"),
            WallMs::from_millis(30),
        );
        assert_eq!(
            reports
                .descriptor(provider)
                .expect("provider answer")
                .status,
            runtrol_runtime_protocol::ProviderAccountStatus::Unpublished
        );
    }

    /// An instant `seconds` before `now`, for a table written the way a person reads it.
    fn ago(now: WallMs, seconds: u64) -> WallMs {
        WallMs::from_millis(now.as_millis() - seconds * 1_000)
    }

    #[test]
    fn a_terminal_that_wrote_and_went_quiet_is_asked_about() {
        // The moment the number moved. Before this the strip waited out a clock instead.
        let now = WallMs::from_millis(1_000_000_000);
        assert!(is_due(Some(ago(now, 5)), None, now, PROCESS_SERVICE_FLOOR));
    }

    #[test]
    fn a_terminal_still_writing_is_not_a_finished_turn() {
        // Mid-turn the CLI writes continuously. Asking then would spend a question on a number about to
        // change again, and would do it for every frame it drew.
        let now = WallMs::from_millis(1_000_000_000);
        assert!(!is_due(
            Some(WallMs::from_millis(now.as_millis() - 500)),
            None,
            now,
            PROCESS_SERVICE_FLOOR
        ));
    }

    #[test]
    fn a_terminal_that_has_written_nothing_has_finished_nothing() {
        let now = WallMs::from_millis(1_000_000_000);
        assert!(!is_due(None, None, now, PROCESS_SERVICE_FLOOR));
    }

    #[test]
    fn nothing_written_since_the_last_answer_needs_no_new_answer() {
        // The quiet is the same quiet that was already asked about. Without this the loop would ask every
        // tick for as long as a finished conversation stayed open.
        let now = WallMs::from_millis(1_000_000_000);
        assert!(!is_due(
            Some(ago(now, 60)),
            Some(ago(now, 30)),
            now,
            PROCESS_SERVICE_FLOOR
        ));
    }

    #[test]
    fn two_short_turns_in_a_row_cost_one_question() {
        // Measured: one question costs a child process peaking near 470 MiB for eight seconds. A person
        // taking twenty-second turns would otherwise pay that on each one.
        let now = WallMs::from_millis(1_000_000_000);
        assert!(!is_due(
            Some(ago(now, 4)),
            Some(ago(now, 10)),
            now,
            PROCESS_SERVICE_FLOOR
        ));
        // And once the floor has passed, the newer turn is asked about.
        assert!(is_due(
            Some(ago(now, 4)),
            Some(ago(now, 40)),
            now,
            PROCESS_SERVICE_FLOOR
        ));
    }

    #[test]
    fn a_structured_account_surface_can_refresh_again_after_five_seconds() {
        let now = WallMs::from_millis(1_000_000_000);
        assert!(is_due(
            Some(ago(now, 2)),
            Some(ago(now, 6)),
            now,
            PROTOCOL_SERVICE_FLOOR
        ));
        assert!(!is_due(
            Some(ago(now, 2)),
            Some(ago(now, 6)),
            now,
            PROCESS_SERVICE_FLOOR
        ));
    }

    #[tokio::test]
    async fn wake_bursts_keep_only_provider_identities() {
        let wake = AccountProbeWake::default();
        let first = ProviderId::parse("first").expect("provider id");
        let second = ProviderId::parse("second").expect("provider id");
        wake.provider(first).await;
        wake.provider(first).await;
        wake.provider(second).await;
        let request = wake.wait().await;
        assert_eq!(request.providers, BTreeSet::from([first, second]));
        assert!(!request.all);

        wake.provider(first).await;
        wake.all().await;
        let request = wake.wait().await;
        assert!(request.all);
        assert_eq!(request.providers, BTreeSet::from([first]));
    }

    #[test]
    fn a_throttled_external_turn_stays_pending_until_its_floor() {
        let provider = ProviderId::parse("outside").expect("provider id");
        let asked_at = WallMs::from_millis(1_000_000_000);
        let mut pending = BTreeSet::from([provider]);
        let asked = BTreeMap::from([(provider, asked_at)]);

        let early = take_due(
            &mut pending,
            &asked,
            WallMs::from_millis(asked_at.as_millis() + 10_000),
            |_| PROCESS_SERVICE_FLOOR,
            |_| true,
        );
        assert!(early.is_empty());
        assert_eq!(pending, BTreeSet::from([provider]));

        let ready = take_due(
            &mut pending,
            &asked,
            WallMs::from_millis(asked_at.as_millis() + 30_000),
            |_| PROCESS_SERVICE_FLOOR,
            |_| true,
        );
        assert_eq!(ready, vec![provider]);
        assert!(pending.is_empty());
    }
}
