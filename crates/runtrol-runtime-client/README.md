# Runtrol Runtime Rust client

This crate connects to one installed per-user Runtime without starting or bundling it. It negotiates the public
revision, proves a consumer-owned integration identity, and exposes typed provider and session operation groups.

Version 0.1.1 implements finalized protocol revision `2026-08-13` and is tested with Runtime 0.1.1. Crate SemVer and
wire revision negotiation are independent compatibility checks.

`SessionClient::watch_index` turns one connection into a dedicated stream. Its acknowledgement contains the initial
authorized snapshot, later notifications carry only changed complete snapshots, and authority loss has a typed final
reason. The SDK performs no polling.

`ProviderClient::watch` follows the same dedicated connection contract for structural provider installation
observations. Runtime publishes only changed verified snapshots and never starts every provider to produce a list.

`SessionClient::forget` first returns `presenceRequired`. The operator approves that exact request from Runtrol Studio,
then the consumer retries the unchanged request identity. Runtime removes only its pointer and never provider state.

A session open with `SessionWorkspaceAccess::Shared` (a second writer in a working tree) follows the same step: `start`,
`adopt_native` and `resume` first return `presenceRequired`, the operator allows that exact open from Runtrol Studio, and
the unchanged request then opens the session.

`IntegrationClient::rotate_key` proves ownership of a replacement Ed25519 key and then returns `presenceRequired`.
Keep the replacement identity, mutation request ID, and previous key generation until the operator confirms the exact
integration ID and replacement fingerprint in Runtrol Studio. Retry those unchanged values to receive credentials for
the incremented key generation. The previous key stops authenticating as soon as the rotation commits.

`RuntimeLocator::connect_with_retry` retries only connection establishment with capped exponential backoff, jitter,
and a total deadline. It reads the validated locator again for every attempt so a restarted Runtime can publish a new
endpoint. Authentication, protocol, and authorization failures return immediately.

`RuntimeLocator::watch_events_with_reconnect` applies the same bound to a dedicated read-only event stream. Call
`accept` with the exact `next_expected` cursor after consuming each event. A replacement connection resumes only from
that accepted cursor, and its `Reconnected` item carries the complete `WatchEventsResult`, including any replay gap.
The wrapper never acquires control or retries input, approval, interrupt, or lifecycle mutations.

`watch_providers_with_reconnect` and `watch_session_index_with_reconnect` replace lost read-only snapshot streams and
surface the new complete snapshot as `Reconnected`. A typed end reason remains terminal and is never changed into a
silent retry.

After local enrollment approval, a consumer can reconnect with its credentials and start a provider-neutral session:

```rust,no_run
# async fn example(
#     runtime: &mut runtrol_runtime_client::RuntimeClient,
#     approved_workspace: String,
# ) -> Result<(), Box<dyn std::error::Error>> {
let provider = runtime
    .providers()
    .list()
    .await?
    .providers
    .into_iter()
    .next()
    .ok_or_else(|| std::io::Error::other("No provider is installed"))?;
let capabilities = runtime
    .providers()
    .get_capabilities(provider.provider_id.clone())
    .await?;
if capabilities.fresh_session.availability
    != runtrol_runtime_protocol::ProviderCapabilityAvailability::Available
{
    return Err(std::io::Error::other("Provider cannot start a fresh session").into());
}
let opened = runtime
    .sessions()
    .start(&runtrol_runtime_protocol::StartSessionParams {
        request_id: runtrol_runtime_protocol::MutationRequestId::now(),
        provider_id: provider.provider_id,
        workspace: approved_workspace,
        access: runtrol_runtime_protocol::SessionWorkspaceAccess::Exclusive,
        model: None,
    })
    .await?;

println!("{}", opened.session.session_id);
let current = runtime
    .sessions()
    .get(opened.session.session_id.clone())
    .await?;
assert_eq!(current.session_id, opened.session.session_id);
runtime
    .sessions()
    .cool(&runtrol_runtime_protocol::CoolSessionParams {
        request_id: runtrol_runtime_protocol::MutationRequestId::now(),
        session_id: opened.session.session_id,
        expected_session_generation: opened.session.session_generation,
        lease_id: opened.control.lease_id,
        lease_generation: opened.control.lease_generation,
    })
    .await?;
# Ok(())
# }
```

Approval risk and option availability are derived from the request held by the live provider driver. A consumer
echoes only the exact identifiers and digest returned by `list_pending`:

```rust,no_run
# async fn approvals(
#     runtime: &mut runtrol_runtime_client::RuntimeClient,
#     control: runtrol_runtime_protocol::ControlLease,
# ) -> Result<(), Box<dyn std::error::Error>> {
let pending = runtime
    .approvals()
    .list_pending(&runtrol_runtime_protocol::ListPendingApprovalsParams {
        session_id: control.session_id.clone(),
        lease_id: control.lease_id.clone(),
        lease_generation: control.lease_generation,
    })
    .await?;
if let Some(approval) = pending.approvals.first()
    && let Some(option) = approval
        .options
        .iter()
        .find(|candidate| candidate.unavailable.is_none())
{
    runtime
        .approvals()
        .respond(&runtrol_runtime_protocol::RespondApprovalParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            session_id: control.session_id,
            lease_id: control.lease_id,
            lease_generation: control.lease_generation,
            approval_id: approval.approval_id.clone(),
            option_id: option.option_id,
            subject_digest: approval.subject_digest,
        })
        .await?;
}
# Ok(())
# }
```

Keep the same mutation request identity when retrying an uncertain transport outcome. Runtime returns the exact known
result, an idempotency conflict, or an explicit unknown outcome. It never reruns an ambiguous provider mutation
automatically.
