# Runtrol Runtime Rust client

This crate connects to one installed per-user Runtime without starting or bundling it. It negotiates the public
revision, proves a consumer-owned integration identity, and exposes typed provider and session operation groups.

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
# Ok(())
# }
```

Keep the same mutation request identity when retrying an uncertain transport outcome. Runtime returns the exact known
result, an idempotency conflict, or an explicit unknown outcome. It never reruns an ambiguous provider mutation
automatically.
