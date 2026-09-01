//! Asking each installed coding service where the operator's account stands, on the service's own
//! status surface, and remembering the answer for the provider and usage projections.
//!
//! Starts on the first account subscriber or provider activity signal, then runs on a slow clock. Nothing here
//! reads a credential file or a transcript: a driver either has a published status surface (`claude auth status
//! --json`, Codex `account/read`) or says it has none, and the projections repeat exactly that.

use std::collections::{BTreeMap, BTreeSet};
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
/// shuts down again. A bound near that fourteen turns a slow network into "this service publishes nothing",
/// which takes the sign-in state off the row along with the limits. Nothing waits behind it: the services
/// are asked at the same time.
const ACCOUNT_PROBE_DEADLINE: Duration = Duration::from_mins(1);
/// How often every service is asked with nothing else prompting it: the backstop, not the driver.
///
/// The rounds that matter are the ones a service's own terminal prompts, so this is the slow sweep that
/// catches what happened somewhere else: a turn taken on the operator's phone, a plan changed in a browser,
/// a limit that reset while nothing was open here.
const ROUND_INTERVAL: Duration = Duration::from_mins(10);
/// How long a service's terminal has to be quiet before its answer is worth asking for again.
///
/// A conversation held as its CLI's own terminal has no turn boundary anybody can subscribe to. What it has
/// is a CLI that writes continuously while it works and then stops, so quiet is the boundary. Long enough
/// that a pause between two frames is not read as an ending, short enough that the strip moves while the
/// person is still looking at it.
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
                let limit = reported.report.as_rate_limit()?;
                Some(runtrol_core::ProviderGauge {
                    provider: *provider,
                    reached: limit.reached,
                    windows: limit.windows,
                    cost: None,
                    tokens_today: reported.report.tokens_today,
                    at: reported.at,
                })
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
    let mut asked: BTreeMap<ProviderId, WallMs> = BTreeMap::new();
    let mut unread: BTreeSet<ProviderId> = BTreeSet::new();
    let mut pending: BTreeSet<ProviderId> = BTreeSet::new();
    let first = composed.account_probe_wake.wait().await;
    let mut first = settled_request(&composed.account_probe_wake, first).await;
    // A request which raced the first-round clock belongs to the same round.
    first.merge(composed.account_probe_wake.take().await);
    let now = WallMs::now();
    pending.extend(requested(&composed, &first));
    let first_ids = take_requested_due(&composed, &mut pending, &asked, now);
    for id in &first_ids {
        // ok: this is the first instant for this service.
        asked.insert(*id, now);
    }
    let first_unread = ask_all(&composed, &providers, &usage, first_ids.clone()).await;
    update_unread(&mut unread, &first_ids, &first_unread);
    // The slow sweep is a maintenance promise made only after somebody first asks for account state. A daemon
    // with no consumer stays process-free here however long it runs.
    let mut swept_at = now;

    loop {
        let wait = next_check(
            &composed,
            &asked,
            &unread,
            &pending,
            swept_at,
            WallMs::now(),
        );
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            request = composed.account_probe_wake.wait() => {
                let request = settled_request(&composed.account_probe_wake, request).await;
                let now = WallMs::now();
                pending.extend(requested(&composed, &request));
                let ids = take_requested_due(&composed, &mut pending, &asked, now);
                for id in &ids {
                    // ok: the previous instant for this service is exactly what is being replaced.
                    asked.insert(*id, now);
                }
                let answer = ask_all(&composed, &providers, &usage, ids.clone()).await;
                update_unread(&mut unread, &ids, &answer);
                if ids.len() == usable(&composed).len() {
                    swept_at = now;
                }
                continue;
            }
        }
        let now = WallMs::now();
        let mut due = due_now(&composed, &asked, swept_at, now).await;
        due.extend(take_requested_due(&composed, &mut pending, &asked, now));
        due.sort_unstable();
        due.dedup();
        // A question that did not come back is the one absence that is runtrol's own, so it is not left to
        // the slow sweep. Which services those are is what the last round answered, kept here rather than
        // read back out of the report table: this runs every couple of seconds, and the projection that
        // builds provider descriptors takes that same table without waiting. Contending with it on a short
        // clock made it publish descriptors with no account at all, and cache them.
        for id in &unread {
            if !due.contains(id)
                && asked
                    .get(id)
                    .is_none_or(|at| at.millis_until(now).unwrap_or(0) >= millis(RETRY_AFTER))
            {
                due.push(*id);
            }
        }
        if due.is_empty() {
            continue;
        }
        if due.len() == usable(&composed).len() {
            swept_at = now;
        }
        for id in &due {
            // ok: the previous instant for this service is exactly what is being replaced.
            asked.insert(*id, now);
        }
        let answered = ask_all(&composed, &providers, &usage, due.clone()).await;
        update_unread(&mut unread, &due, &answered);
    }
}

async fn settled_request(wake: &AccountProbeWake, mut request: ProbeRequest) -> ProbeRequest {
    tokio::time::sleep(WAKE_SETTLE).await;
    request.merge(wake.take().await);
    request
}

fn update_unread(
    unread: &mut BTreeSet<ProviderId>,
    asked: &[ProviderId],
    answered_unread: &BTreeSet<ProviderId>,
) {
    for id in asked {
        if answered_unread.contains(id) {
            // ok: the set is the answer, and whether this id was already in it says nothing.
            unread.insert(*id);
        } else {
            unread.remove(id);
        }
    }
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
fn take_requested_due(
    composed: &Composed,
    pending: &mut BTreeSet<ProviderId>,
    asked: &BTreeMap<ProviderId, WallMs>,
    now: WallMs,
) -> Vec<ProviderId> {
    take_due(pending, asked, now, |provider| {
        service_floor(composed, provider)
    })
}

fn take_due(
    pending: &mut BTreeSet<ProviderId>,
    asked: &BTreeMap<ProviderId, WallMs>,
    now: WallMs,
    floor: impl Fn(ProviderId) -> Duration,
) -> Vec<ProviderId> {
    let due: Vec<ProviderId> = pending
        .iter()
        .copied()
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

/// Sleep only until a fact could next be due.
///
/// An open terminal gets the cheap half-second clock that notices quiet. With no terminal and no unread
/// answer, the task sleeps straight to the ten-minute sweep; an idle daemon does not wake twice a second just
/// to discover that it is still idle.
fn next_check(
    composed: &Composed,
    asked: &BTreeMap<ProviderId, WallMs>,
    unread: &BTreeSet<ProviderId>,
    pending: &BTreeSet<ProviderId>,
    swept_at: WallMs,
    now: WallMs,
) -> Duration {
    let remaining = |since: WallMs, interval: Duration| {
        interval.saturating_sub(Duration::from_millis(since.millis_until(now).unwrap_or(0)))
    };
    let mut next = remaining(swept_at, ROUND_INTERVAL);
    if composed
        .open_terminals
        .load(std::sync::atomic::Ordering::Acquire)
        > 0
    {
        next = next.min(WATCH_TICK);
    }
    for provider in unread {
        if let Some(at) = asked.get(provider) {
            next = next.min(remaining(*at, RETRY_AFTER));
        }
    }
    for provider in pending {
        next = next.min(asked.get(provider).map_or(Duration::ZERO, |at| {
            remaining(*at, service_floor(composed, *provider))
        }));
    }
    next
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
        return usable();
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

/// Ask exactly these services and republish what changed.
#[expect(
    clippy::print_stderr,
    reason = "a detached provider inventory rebuild has no request to answer; stderr is the daemon's operational failure channel"
)]
async fn ask_all(
    composed: &Arc<Composed>,
    providers: &watch::Sender<Arc<runtrol_runtime_protocol::ProviderList>>,
    usage: &watch::Sender<Arc<runtrol_runtime_protocol::ProviderUsageList>>,
    ids: Vec<ProviderId>,
) -> BTreeSet<ProviderId> {
    if ids.is_empty() {
        return BTreeSet::new();
    }

    // All at once, because the services have nothing to do with each other. Asked one after another a round
    // took as long as the answers added up, and every service behind the slow one had a standing bar for
    // those seconds; asked together it takes as long as the slowest, and one service being slow costs only
    // that service. It is also what lets the deadline above be generous without anything else paying for it.
    let mut asking = tokio::task::JoinSet::new();
    for id in ids {
        let composed = Arc::clone(composed);
        drop(asking.spawn(async move { (id, ask(&composed, id).await) }));
    }
    let mut answers = Vec::new();
    while let Some(joined) = asking.join_next().await {
        // Two ways to come back with nothing, and neither writes a report. A service that could not be
        // prepared is an installation problem the inventory already names, and a task that did not finish
        // is a reading nobody took. In both the row keeps its last answer and the next round asks again.
        if let Ok((id, Some(report))) = joined {
            answers.push((id, report));
        }
    }

    let now = WallMs::now();
    let mut changed = false;
    let mut unread = BTreeSet::new();
    {
        let mut reports = composed.account_reports.lock().await;
        for (id, report) in answers {
            if report
                .limits_absent
                .as_ref()
                .is_some_and(runtrol_provider::LimitsAbsent::is_worth_retrying)
            {
                // ok: the set is the answer, and whether this id was already in it says nothing.
                unread.insert(id);
            }
            let same = reports.get(id).is_some_and(|known| known.report == report);
            reports.record(id, report, now);
            changed |= !same;
        }
    }
    // EmptyWorkingSet evicts the whole daemon, including the PTY hot path. It is useful only while idle;
    // active terminals keep their resident pages so a background account round cannot tax keystrokes.
    if composed
        .open_terminals
        .load(std::sync::atomic::Ordering::Acquire)
        == 0
    {
        runtrol_childproc::footprint::release_unused_memory();
    }
    if !changed {
        return unread;
    }
    // The provider descriptors carry the status and the usage list carries the probed windows; both
    // projections read the reports on their own, so publishing is asking each to look again.
    crate::runtime_inventory::invalidate_provider_inventory(composed).await;
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
    usage.send_modify(|current| {
        let merged = crate::runtime_inventory::merge_probed_usage(current.as_ref(), composed);
        *current = Arc::new(merged);
    });
    unread
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

#[cfg(test)]
mod tests {
    use super::*;

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
        );
        assert!(early.is_empty());
        assert_eq!(pending, BTreeSet::from([provider]));

        let ready = take_due(
            &mut pending,
            &asked,
            WallMs::from_millis(asked_at.as_millis() + 30_000),
            |_| PROCESS_SERVICE_FLOOR,
        );
        assert_eq!(ready, vec![provider]);
        assert!(pending.is_empty());
    }
}
