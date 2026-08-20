//! Root-authorized provider-native catalogue filtering and boot-local cursor authenticity.

use std::collections::BTreeSet;

use base64ct::{Base64UrlUnpadded, Encoding as _};
use hmac::{Hmac, Mac as _};
use runtrol_provider::{
    AbsPath, NativeCatalogueCoverage as ProviderCoverage, NativeCatalogueSource as ProviderSource,
    NativeResumeCapability as ProviderResume, NativeSessionCatalogue as ProviderCatalogue,
    ProviderId as CoreProviderId, WallMs,
};
use runtrol_runtime_protocol::{
    CatalogueCoverage, CatalogueSource, MAX_NATIVE_ADOPTION_TOKEN_BYTES,
    MAX_NATIVE_PUBLIC_CURSOR_BYTES, NATIVE_CURSOR_LIFETIME_MS, NativeResumeCapability,
    NativeSessionCatalogue, NativeSessionDescriptor, ProviderId,
};
use sha2::{Digest as _, Sha256};

use crate::runtime_auth::AuthorizedIntegration;
use crate::runtime_inventory::{AuthorizedRoot, RuntimeInventoryFailure, RuntimeSessionCatalogue};

const CURSOR_VERSION: u8 = 1;
const ADOPTION_VERSION: u8 = 2;
const CURSOR_TAG_BYTES: usize = 32;
const MAX_CURSOR_PAGES: usize = 32;
const MAX_REASON_BYTES: usize = 4 * 1024;

/// One decoded provider cursor plus the cursor hashes already seen in this walk.
pub(crate) struct OpenedCursor {
    pub(crate) provider_cursor: Box<str>,
    seen: Vec<[u8; 32]>,
}

/// A boot-local codec. Restarting Runtime intentionally invalidates every slow-discovery cursor.
pub(crate) struct NativeCursorCodec {
    authenticator: Hmac<Sha256>,
}

/// A public cursor was unsafe or no longer names its original discovery context.
#[derive(Clone, Copy, Debug)]
pub(crate) enum NativeCursorFailure {
    Invalid,
    Expired,
    TooManyPages,
    Internal,
}

/// How a page's scope is bound into its cursor: one folder, or the whole machine.
///
/// A cursor carries the context it was issued for so it cannot be replayed into a different one.
/// That mattered when every page was one folder (a folder-A cursor handed to a folder-B listing
/// would have sent A's provider cursor into B's call), and it still matters now that a page can
/// cover the machine: a machine-wide cursor must not be accepted for a folder listing, or the
/// other way round. So the scope is always signed and the machine simply has its own constant.
///
/// The constant is a fixed sentence rather than zeroes so it cannot collide with a real folder.
fn scope_bytes(root: Option<&AuthorizedRoot>) -> ([u8; 32], [u8; 24]) {
    match root {
        Some(root) => (digest(root.path.as_str()), root.identity),
        None => (
            digest("machine-wide native catalogue, no folder filter"),
            [0_u8; 24],
        ),
    }
}

impl NativeCursorCodec {
    /// Mint one unpredictable boot-local cursor authenticity key.
    pub(crate) fn new() -> Result<Self, NativeCursorFailure> {
        let mut key = [0_u8; 32];
        getrandom::fill(&mut key).map_err(|_| NativeCursorFailure::Internal)?;
        let authenticator =
            Hmac::<Sha256>::new_from_slice(&key).map_err(|_| NativeCursorFailure::Internal)?;
        Ok(Self { authenticator })
    }

    /// Authenticate and bind one provider cursor to its integration, root, grant, and exact binary identity.
    pub(crate) fn seal(
        &self,
        authority: &AuthorizedIntegration,
        provider: CoreProviderId,
        root: Option<&AuthorizedRoot>,
        binary_identity: [u8; 32],
        provider_cursor: &str,
        prior: Option<&OpenedCursor>,
    ) -> Result<String, NativeCursorFailure> {
        if provider_cursor.len() > runtrol_provider::MAX_NATIVE_CURSOR_BYTES {
            return Err(NativeCursorFailure::Invalid);
        }
        let cursor_hash: [u8; 32] = Sha256::digest(provider_cursor.as_bytes()).into();
        let mut seen = prior.map_or_else(Vec::new, |opened| opened.seen.clone());
        if seen.contains(&cursor_hash) {
            return Err(NativeCursorFailure::Invalid);
        }
        if seen.len() >= MAX_CURSOR_PAGES {
            return Err(NativeCursorFailure::TooManyPages);
        }
        seen.push(cursor_hash);

        let mut body = Vec::with_capacity(192_usize.saturating_add(provider_cursor.len()));
        body.push(CURSOR_VERSION);
        body.extend_from_slice(&authority.key.to_bytes());
        body.extend_from_slice(&authority.grant.key_generation.to_be_bytes());
        body.extend_from_slice(&authority.grant.grant_generation.to_be_bytes());
        body.extend_from_slice(&digest(provider.as_str()));
        let (scope, identity) = scope_bytes(root);
        body.extend_from_slice(&scope);
        body.extend_from_slice(&identity);
        body.extend_from_slice(&binary_identity);
        body.extend_from_slice(
            &WallMs::now()
                .as_millis()
                .saturating_add(NATIVE_CURSOR_LIFETIME_MS)
                .to_be_bytes(),
        );
        let length =
            u16::try_from(provider_cursor.len()).map_err(|_| NativeCursorFailure::Invalid)?;
        body.extend_from_slice(&length.to_be_bytes());
        body.extend_from_slice(provider_cursor.as_bytes());
        body.push(u8::try_from(seen.len()).map_err(|_| NativeCursorFailure::TooManyPages)?);
        for hash in seen {
            body.extend_from_slice(&hash);
        }
        let mut mac = self.authenticator.clone();
        mac.update(&body);
        body.extend_from_slice(&mac.finalize().into_bytes());
        let encoded = Base64UrlUnpadded::encode_string(&body);
        if encoded.len() > MAX_NATIVE_PUBLIC_CURSOR_BYTES {
            return Err(NativeCursorFailure::Invalid);
        }
        Ok(encoded)
    }

    /// Open one cursor only in the exact context in which Runtime issued it.
    pub(crate) fn open(
        &self,
        authority: &AuthorizedIntegration,
        provider: CoreProviderId,
        root: Option<&AuthorizedRoot>,
        binary_identity: [u8; 32],
        encoded: &str,
    ) -> Result<OpenedCursor, NativeCursorFailure> {
        if encoded.len() > MAX_NATIVE_PUBLIC_CURSOR_BYTES {
            return Err(NativeCursorFailure::Invalid);
        }
        let decoded =
            Base64UrlUnpadded::decode_vec(encoded).map_err(|_| NativeCursorFailure::Invalid)?;
        let body_length = decoded
            .len()
            .checked_sub(CURSOR_TAG_BYTES)
            .ok_or(NativeCursorFailure::Invalid)?;
        let (body, tag) = decoded.split_at(body_length);
        let mut mac = self.authenticator.clone();
        mac.update(body);
        mac.verify_slice(tag)
            .map_err(|_| NativeCursorFailure::Invalid)?;
        decode_body(body, authority, provider, root, binary_identity)
    }

    /// Bind one adoptable observation to its integration, provider binary, approved root, identity, and workspace.
    pub(crate) fn seal_adoption(
        &self,
        authority: &AuthorizedIntegration,
        provider: CoreProviderId,
        root: Option<&AuthorizedRoot>,
        binary_identity: [u8; 32],
        native_session_id: &str,
        workspace: &AbsPath,
    ) -> Result<String, NativeCursorFailure> {
        if native_session_id.len() > runtrol_provider::MAX_NATIVE_CURSOR_BYTES {
            return Err(NativeCursorFailure::Invalid);
        }
        let mut body = Vec::with_capacity(224_usize.saturating_add(native_session_id.len()));
        body.push(ADOPTION_VERSION);
        body.extend_from_slice(&authority.key.to_bytes());
        body.extend_from_slice(&authority.grant.key_generation.to_be_bytes());
        body.extend_from_slice(&authority.grant.grant_generation.to_be_bytes());
        body.extend_from_slice(&digest(provider.as_str()));
        let (scope, identity) = scope_bytes(root);
        body.extend_from_slice(&scope);
        body.extend_from_slice(&identity);
        body.extend_from_slice(&binary_identity);
        body.extend_from_slice(&digest(workspace.as_str()));
        body.extend_from_slice(
            &WallMs::now()
                .as_millis()
                .saturating_add(NATIVE_CURSOR_LIFETIME_MS)
                .to_be_bytes(),
        );
        let length =
            u16::try_from(native_session_id.len()).map_err(|_| NativeCursorFailure::Invalid)?;
        body.extend_from_slice(&length.to_be_bytes());
        body.extend_from_slice(native_session_id.as_bytes());
        let mut mac = self.authenticator.clone();
        mac.update(&body);
        body.extend_from_slice(&mac.finalize().into_bytes());
        let encoded = Base64UrlUnpadded::encode_string(&body);
        if encoded.len() > MAX_NATIVE_ADOPTION_TOKEN_BYTES {
            return Err(NativeCursorFailure::Invalid);
        }
        Ok(encoded)
    }

    /// Verify one adoption proof only against current grant roots and the exact prepared provider binary.
    #[expect(
        clippy::too_many_arguments,
        reason = "adoption proof verification binds every independent authority and provider observation dimension"
    )]
    pub(crate) fn open_adoption(
        &self,
        authority: &AuthorizedIntegration,
        approved_roots: &[AuthorizedRoot],
        provider: CoreProviderId,
        binary_identity: [u8; 32],
        native_session_id: &str,
        workspace: &AbsPath,
        encoded: &str,
    ) -> Result<(), NativeCursorFailure> {
        if encoded.len() > MAX_NATIVE_ADOPTION_TOKEN_BYTES {
            return Err(NativeCursorFailure::Invalid);
        }
        let decoded =
            Base64UrlUnpadded::decode_vec(encoded).map_err(|_| NativeCursorFailure::Invalid)?;
        let body_length = decoded
            .len()
            .checked_sub(CURSOR_TAG_BYTES)
            .ok_or(NativeCursorFailure::Invalid)?;
        let (body, tag) = decoded.split_at(body_length);
        let mut mac = self.authenticator.clone();
        mac.update(body);
        mac.verify_slice(tag)
            .map_err(|_| NativeCursorFailure::Invalid)?;
        decode_adoption(
            body,
            authority,
            approved_roots,
            provider,
            binary_identity,
            native_session_id,
            workspace,
        )
    }
}

fn decode_adoption(
    body: &[u8],
    authority: &AuthorizedIntegration,
    approved_roots: &[AuthorizedRoot],
    provider: CoreProviderId,
    binary_identity: [u8; 32],
    native_session_id: &str,
    workspace: &AbsPath,
) -> Result<(), NativeCursorFailure> {
    let mut cursor = Cursor { bytes: body, at: 0 };
    if cursor.byte()? != ADOPTION_VERSION
        || cursor.fixed::<16>()? != authority.key.to_bytes()
        || cursor.u64()? != authority.grant.key_generation
        || cursor.u64()? != authority.grant.grant_generation
        || cursor.fixed::<32>()? != digest(provider.as_str())
    {
        return Err(NativeCursorFailure::Invalid);
    }
    let root_digest = cursor.fixed::<32>()?;
    let root_identity = cursor.fixed::<24>()?;
    // A proof sealed for a named folder opens only while that folder is still an approved root. A proof
    // sealed by the machine-wide listing carries the machine scope instead of a folder: that listing is
    // answered only on the owner-only local endpoint, the proof is bound to this integration's key, this
    // provider binary, this exact workspace and a five-minute life, so it opens on the same terms the
    // listing was given. Measured before this arm existed: every conversation the machine-wide list showed
    // refused to open, because no approved root could ever match the machine scope.
    let (machine_scope, machine_identity) = scope_bytes(None);
    let machine_wide = root_digest == machine_scope && root_identity == machine_identity;
    let folder_approved = approved_roots
        .iter()
        .any(|root| digest(root.path.as_str()) == root_digest && root.identity == root_identity);
    if !(machine_wide || folder_approved)
        || cursor.fixed::<32>()? != binary_identity
        || cursor.fixed::<32>()? != digest(workspace.as_str())
    {
        return Err(NativeCursorFailure::Invalid);
    }
    if cursor.u64()? <= WallMs::now().as_millis() {
        return Err(NativeCursorFailure::Expired);
    }
    let native_length = usize::from(cursor.u16()?);
    if native_length > runtrol_provider::MAX_NATIVE_CURSOR_BYTES
        || cursor.take(native_length)? != native_session_id.as_bytes()
        || cursor.at != body.len()
    {
        return Err(NativeCursorFailure::Invalid);
    }
    Ok(())
}

fn decode_body(
    body: &[u8],
    authority: &AuthorizedIntegration,
    provider: CoreProviderId,
    root: Option<&AuthorizedRoot>,
    binary_identity: [u8; 32],
) -> Result<OpenedCursor, NativeCursorFailure> {
    let (scope, identity) = scope_bytes(root);
    let mut cursor = Cursor { bytes: body, at: 0 };
    if cursor.byte()? != CURSOR_VERSION
        || cursor.fixed::<16>()? != authority.key.to_bytes()
        || cursor.u64()? != authority.grant.key_generation
        || cursor.u64()? != authority.grant.grant_generation
        || cursor.fixed::<32>()? != digest(provider.as_str())
        // Scope, not just folder: a machine-wide cursor and a folder cursor cannot be swapped.
        || cursor.fixed::<32>()? != scope
        || cursor.fixed::<24>()? != identity
        || cursor.fixed::<32>()? != binary_identity
    {
        return Err(NativeCursorFailure::Invalid);
    }
    if cursor.u64()? <= WallMs::now().as_millis() {
        return Err(NativeCursorFailure::Expired);
    }
    let provider_length = usize::from(cursor.u16()?);
    if provider_length > runtrol_provider::MAX_NATIVE_CURSOR_BYTES {
        return Err(NativeCursorFailure::Invalid);
    }
    let provider_cursor = std::str::from_utf8(cursor.take(provider_length)?)
        .map_err(|_| NativeCursorFailure::Invalid)?;
    let seen_count = usize::from(cursor.byte()?);
    if seen_count == 0 || seen_count > MAX_CURSOR_PAGES {
        return Err(NativeCursorFailure::Invalid);
    }
    let mut seen = Vec::with_capacity(seen_count);
    for _ in 0..seen_count {
        seen.push(cursor.fixed::<32>()?);
    }
    if cursor.at != body.len()
        || seen.last().copied() != Some(Sha256::digest(provider_cursor.as_bytes()).into())
    {
        return Err(NativeCursorFailure::Invalid);
    }
    Ok(OpenedCursor {
        provider_cursor: provider_cursor.into(),
        seen,
    })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], NativeCursorFailure> {
        let end = self
            .at
            .checked_add(length)
            .ok_or(NativeCursorFailure::Invalid)?;
        let value = self
            .bytes
            .get(self.at..end)
            .ok_or(NativeCursorFailure::Invalid)?;
        self.at = end;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8, NativeCursorFailure> {
        self.take(1)?
            .first()
            .copied()
            .ok_or(NativeCursorFailure::Invalid)
    }

    fn u16(&mut self) -> Result<u16, NativeCursorFailure> {
        Ok(u16::from_be_bytes(self.fixed()?))
    }

    fn u64(&mut self) -> Result<u64, NativeCursorFailure> {
        Ok(u64::from_be_bytes(self.fixed()?))
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], NativeCursorFailure> {
        self.take(N)?
            .try_into()
            .map_err(|_| NativeCursorFailure::Invalid)
    }
}

/// Convert one provider page only after every path is canonical and inside current approved roots.
#[expect(
    clippy::too_many_arguments,
    reason = "catalogue authorization binds caller, root, provider, binary, workspace, managed merge, and proof authority"
)]
pub(crate) fn authorize_catalogue(
    codec: &NativeCursorCodec,
    authority: &AuthorizedIntegration,
    selected_root: Option<&AuthorizedRoot>,
    approved_roots: &[AuthorizedRoot],
    managed: &RuntimeSessionCatalogue,
    provider: CoreProviderId,
    binary_identity: [u8; 32],
    catalogue: ProviderCatalogue,
) -> Result<NativeSessionCatalogue, RuntimeInventoryFailure> {
    if catalogue.sessions.len() > runtrol_provider::MAX_NATIVE_SESSION_ITEMS
        || catalogue.next_cursor.as_deref().is_some_and(|cursor| {
            cursor.len() > runtrol_provider::MAX_NATIVE_CURSOR_BYTES
                || cursor.chars().any(char::is_control)
        })
        || matches!(&catalogue.coverage, ProviderCoverage::Unsupported { .. })
            && (!catalogue.sessions.is_empty() || catalogue.next_cursor.is_some())
    {
        return Err(RuntimeInventoryFailure::Unavailable);
    }
    let mut omitted = 0_usize;
    let mut seen = BTreeSet::new();
    let mut sessions = Vec::with_capacity(catalogue.sessions.len());
    for entry in catalogue.sessions {
        if entry.additional_directories.len() > runtrol_provider::MAX_NATIVE_ADDITIONAL_DIRECTORIES
            || entry.title.as_deref().is_some_and(|title| {
                title.len() > runtrol_provider::MAX_NATIVE_TITLE_BYTES
                    || title.chars().any(char::is_control)
            })
            || entry.updated_at.as_deref().is_some_and(|timestamp| {
                timestamp.len() > runtrol_provider::MAX_NATIVE_TIMESTAMP_BYTES
                    || timestamp.chars().any(char::is_control)
            })
        {
            omitted = omitted.saturating_add(1);
            continue;
        }
        if !seen.insert(entry.native.as_str().to_owned()) {
            omitted = omitted.saturating_add(1);
            continue;
        }
        let Ok(cwd) = AbsPath::canonicalize(&entry.cwd) else {
            omitted = omitted.saturating_add(1);
            continue;
        };
        // A named folder keeps its old boundary: the provider was asked about that folder, and a
        // row outside it would be a provider answering something else. A machine-wide request has
        // no such boundary to enforce, which is the point of it.
        if selected_root.is_some_and(|selected| !cwd.is_under(&selected.path)) {
            omitted = omitted.saturating_add(1);
            continue;
        }
        let additional = entry
            .additional_directories
            .iter()
            .map(|path| AbsPath::canonicalize(path))
            .collect::<Result<Vec<_>, _>>();
        let Ok(additional) = additional else {
            omitted = omitted.saturating_add(1);
            continue;
        };
        // The same split. Inside a named folder, a session reaching into folders this integration
        // was never granted is dropped rather than shown. A machine-wide listing on the owner-only
        // local endpoint is already looking at the whole machine, so there is no smaller set for
        // an extra directory to escape from.
        if selected_root.is_some()
            && additional
                .iter()
                .any(|path| !approved_roots.iter().any(|root| path.is_under(&root.path)))
        {
            omitted = omitted.saturating_add(1);
            continue;
        }
        let already_managed_as = managed.managed_as(authority, provider, &entry.native)?;
        let resume = map_resume(entry.resume);
        let adoption_token =
            if resume == NativeResumeCapability::Available && already_managed_as.is_none() {
                Some(
                    codec
                        .seal_adoption(
                            authority,
                            provider,
                            selected_root,
                            binary_identity,
                            entry.native.as_str(),
                            &cwd,
                        )
                        .map_err(|_| RuntimeInventoryFailure::Unavailable)?,
                )
            } else {
                None
            };
        sessions.push(NativeSessionDescriptor {
            native_session_id: entry.native.to_string(),
            cwd: cwd.to_string(),
            additional_directories: additional
                .into_iter()
                .map(|path| path.to_string())
                .collect(),
            title: entry.title.map(String::from),
            updated_at: entry.updated_at.map(String::from),
            resume,
            already_managed_as,
            adoption_token,
        });
    }
    let coverage = map_coverage(catalogue.coverage, omitted)?;
    Ok(NativeSessionCatalogue {
        provider_id: ProviderId::new(provider.as_str()),
        coverage,
        sessions,
        next_cursor: None,
    })
}

fn map_coverage(
    coverage: ProviderCoverage,
    omitted: usize,
) -> Result<CatalogueCoverage, RuntimeInventoryFailure> {
    match coverage {
        ProviderCoverage::Complete { source } if omitted == 0 => Ok(CatalogueCoverage::Complete {
            source: map_source(source),
        }),
        ProviderCoverage::Complete { source } => Ok(CatalogueCoverage::Partial {
            source: map_source(source),
            why: omitted_sentence(omitted),
        }),
        ProviderCoverage::Partial { source, why } => {
            if why.len() > MAX_REASON_BYTES || why.chars().any(char::is_control) {
                return Err(RuntimeInventoryFailure::Unavailable);
            }
            let why = if omitted == 0 {
                String::from(why)
            } else {
                format!("{why}; {}", omitted_sentence(omitted))
            };
            Ok(CatalogueCoverage::Partial {
                source: map_source(source),
                why,
            })
        }
        ProviderCoverage::Unsupported { why } => {
            if why.len() > MAX_REASON_BYTES || why.chars().any(char::is_control) || omitted != 0 {
                return Err(RuntimeInventoryFailure::Unavailable);
            }
            Ok(CatalogueCoverage::Unsupported {
                why: String::from(why),
            })
        }
    }
}

/// Why some of what the provider listed is not shown, in words a reader can act on.
///
/// Every omission has one of three causes: the folder the conversation names no longer exists (so nothing
/// could reopen it there), the entry repeats one already shown, or the entry is outside the bounds this
/// surface accepts (or, for a named-folder request, outside that folder). The count is the provider's own.
fn omitted_sentence(omitted: usize) -> String {
    if omitted == 1 {
        "1 listed conversation is not shown: its folder no longer exists, or it repeats or overruns an entry".to_owned()
    } else {
        format!(
            "{omitted} listed conversations are not shown: their folders no longer exist, or they repeat or overrun entries"
        )
    }
}

const fn map_source(source: ProviderSource) -> CatalogueSource {
    match source {
        ProviderSource::OfficialProtocol => CatalogueSource::OfficialProtocol,
        ProviderSource::OfficialCli => CatalogueSource::OfficialCli,
    }
}

const fn map_resume(resume: ProviderResume) -> NativeResumeCapability {
    match resume {
        ProviderResume::Available => NativeResumeCapability::Available,
        ProviderResume::Unavailable => NativeResumeCapability::Unavailable,
        ProviderResume::Unknown => NativeResumeCapability::Unknown,
    }
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use runtrol_provider::{
        NativeCatalogueCoverage, NativeCatalogueSource,
        NativeResumeCapability as ProviderNativeResume, NativeSessionEntry, NativeSessionId,
    };
    use runtrol_runtime_protocol::{AppScope, IntegrationGrant, IntegrationId};
    use runtrol_store::{IntegrationKey, IntegrationRootRow};

    use super::*;

    fn authority(root: &AuthorizedRoot) -> AuthorizedIntegration {
        AuthorizedIntegration {
            key: IntegrationKey::from_bytes([7; 16]),
            grant: IntegrationGrant {
                integration_id: IntegrationId::new("int_fixture"),
                scopes: vec![AppScope::SessionNativeDiscover],
                roots: vec![root.path.to_string()],
                key_generation: 2,
                grant_generation: 3,
            },
            roots: vec![IntegrationRootRow {
                path: root.path.as_str().into(),
                identity: root.identity,
            }],
        }
    }

    #[test]
    fn cursor_is_bound_to_root_provider_binary_and_grant() {
        let root = AuthorizedRoot {
            path: AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" }).expect("absolute"),
            identity: [9; 24],
        };
        let authority = authority(&root);
        let provider = CoreProviderId::parse("provider").expect("valid provider");
        let codec = NativeCursorCodec::new().expect("random codec");
        let token = codec
            .seal(
                &authority,
                provider,
                Some(&root),
                [4; 32],
                "provider-page-2",
                None,
            )
            .expect("sealed");
        let opened = codec
            .open(&authority, provider, Some(&root), [4; 32], &token)
            .expect("same context opens");
        assert_eq!(opened.provider_cursor.as_ref(), "provider-page-2");
        assert!(
            codec
                .open(&authority, provider, Some(&root), [5; 32], &token)
                .is_err()
        );
    }

    #[test]
    fn a_machine_wide_adoption_proof_opens_for_the_same_workspace_whatever_the_roots() {
        // The listing that minted it had no folder; the proof must open on the same terms, bound to the
        // workspace it named, rather than demanding a root that the machine scope can never match.
        let root = AuthorizedRoot {
            path: AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" }).expect("absolute"),
            identity: [9; 24],
        };
        let authority = authority(&root);
        let provider = CoreProviderId::parse("provider").expect("valid provider");
        let codec = NativeCursorCodec::new().expect("random codec");
        let workspace = AbsPath::new(if cfg!(windows) {
            r"C:\elsewhere"
        } else {
            "/elsewhere"
        })
        .expect("absolute");
        let proof = codec
            .seal_adoption(&authority, provider, None, [4; 32], "native-1", &workspace)
            .expect("sealed");
        codec
            .open_adoption(
                &authority,
                &[],
                provider,
                [4; 32],
                "native-1",
                &workspace,
                &proof,
            )
            .expect("a machine-wide proof opens with no approved roots at all");
        codec
            .open_adoption(
                &authority,
                std::slice::from_ref(&root),
                provider,
                [4; 32],
                "native-1",
                &workspace,
                &proof,
            )
            .expect("and with unrelated approved roots");
        let other =
            AbsPath::new(if cfg!(windows) { r"C:\other" } else { "/other" }).expect("absolute");
        assert!(
            codec
                .open_adoption(
                    &authority,
                    &[],
                    provider,
                    [4; 32],
                    "native-1",
                    &other,
                    &proof
                )
                .is_err(),
            "the workspace is still bound"
        );
        assert!(
            codec
                .open_adoption(
                    &authority,
                    &[],
                    provider,
                    [5; 32],
                    "native-1",
                    &workspace,
                    &proof
                )
                .is_err(),
            "and so is the provider binary"
        );
    }

    #[test]
    fn a_folder_adoption_proof_still_needs_its_folder_approved() {
        let root = AuthorizedRoot {
            path: AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" }).expect("absolute"),
            identity: [9; 24],
        };
        let authority = authority(&root);
        let provider = CoreProviderId::parse("provider").expect("valid provider");
        let codec = NativeCursorCodec::new().expect("random codec");
        let proof = codec
            .seal_adoption(
                &authority,
                provider,
                Some(&root),
                [4; 32],
                "native-1",
                &root.path,
            )
            .expect("sealed");
        codec
            .open_adoption(
                &authority,
                std::slice::from_ref(&root),
                provider,
                [4; 32],
                "native-1",
                &root.path,
                &proof,
            )
            .expect("opens while the folder is approved");
        assert!(
            codec
                .open_adoption(
                    &authority,
                    &[],
                    provider,
                    [4; 32],
                    "native-1",
                    &root.path,
                    &proof
                )
                .is_err(),
            "a folder proof does not open once its folder is gone from the grant"
        );
    }

    #[test]
    fn cursor_rejects_a_repeated_provider_page() {
        let root = AuthorizedRoot {
            path: AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" }).expect("absolute"),
            identity: [9; 24],
        };
        let authority = authority(&root);
        let provider = CoreProviderId::parse("provider").expect("valid provider");
        let codec = NativeCursorCodec::new().expect("random codec");
        let first = codec
            .seal(
                &authority,
                provider,
                Some(&root),
                [4; 32],
                "provider-page-2",
                None,
            )
            .expect("first cursor");
        let opened = codec
            .open(&authority, provider, Some(&root), [4; 32], &first)
            .expect("open first");
        assert!(
            codec
                .seal(
                    &authority,
                    provider,
                    Some(&root),
                    [4; 32],
                    "provider-page-2",
                    Some(&opened)
                )
                .is_err()
        );
    }

    /// The one provider answer both scope tests are judged against.
    ///
    /// Two rows inside a folder the integration holds (sharing one native identity, so
    /// deduplication and managed-session merging are exercised) and one row in a folder it does
    /// not. Which of those survive is exactly what the scope decides.
    struct CatalogueFixture {
        base: std::path::PathBuf,
        root: AuthorizedRoot,
        authority: AuthorizedIntegration,
        provider: CoreProviderId,
        managed: RuntimeSessionCatalogue,
        project: AbsPath,
        outside: AbsPath,
    }

    impl CatalogueFixture {
        fn make(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "runtrol-native-catalogue-{name}-{}",
                std::process::id()
            ));
            drop(std::fs::remove_dir_all(&base));
            let project_path = base.join("project");
            let outside_path = base.join("outside");
            std::fs::create_dir_all(&project_path).expect("create project");
            std::fs::create_dir_all(&outside_path).expect("create outside");
            let project = AbsPath::canonicalize(project_path.to_str().expect("UTF-8 project"))
                .expect("canonical project");
            let outside = AbsPath::canonicalize(outside_path.to_str().expect("UTF-8 outside"))
                .expect("canonical outside");
            let root = AuthorizedRoot {
                path: project.clone(),
                identity: runtrol_security::ProjectRootIdentity::read(&project)
                    .expect("root identity")
                    .to_bytes(),
            };
            let authority = authority(&root);
            let provider = CoreProviderId::parse("provider").expect("valid provider");
            let managed = RuntimeSessionCatalogue::one_for_tests(provider, "native-1", &project);
            Self {
                base,
                root,
                authority,
                provider,
                managed,
                project,
                outside,
            }
        }

        fn answered(&self) -> ProviderCatalogue {
            let native = || NativeSessionId::new("native-1").expect("valid native");
            ProviderCatalogue {
                coverage: NativeCatalogueCoverage::Complete {
                    source: NativeCatalogueSource::OfficialProtocol,
                },
                sessions: vec![
                    NativeSessionEntry {
                        native: native(),
                        cwd: self.project.as_str().into(),
                        additional_directories: Vec::new(),
                        title: Some("Provider title".into()),
                        updated_at: None,
                        resume: ProviderNativeResume::Available,
                    },
                    NativeSessionEntry {
                        native: native(),
                        cwd: self.project.as_str().into(),
                        additional_directories: Vec::new(),
                        title: None,
                        updated_at: None,
                        resume: ProviderNativeResume::Available,
                    },
                    NativeSessionEntry {
                        native: NativeSessionId::new("native-outside").expect("valid native"),
                        cwd: self.outside.as_str().into(),
                        additional_directories: Vec::new(),
                        title: None,
                        updated_at: None,
                        resume: ProviderNativeResume::Available,
                    },
                ],
                next_cursor: None,
            }
        }

        fn authorized(&self, scope: Option<&AuthorizedRoot>) -> NativeSessionCatalogue {
            authorize_catalogue(
                &NativeCursorCodec::new().expect("random codec"),
                &self.authority,
                scope,
                std::slice::from_ref(&self.root),
                &self.managed,
                self.provider,
                [4; 32],
                self.answered(),
            )
            .expect("filter catalogue")
        }
    }

    impl Drop for CatalogueFixture {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.base));
        }
    }

    #[test]
    fn a_named_folder_keeps_its_boundary_and_merges_by_native_identity() {
        let fixture = CatalogueFixture::make("folder");
        let public = fixture.authorized(Some(&fixture.root));
        assert_eq!(
            public.sessions.len(),
            1,
            "the row outside the folder is not shown"
        );
        assert!(
            public
                .sessions
                .first()
                .is_some_and(|session| session.already_managed_as.is_some()),
            "the two rows sharing one native identity collapse into the managed one"
        );
        assert!(matches!(
            public.coverage,
            CatalogueCoverage::Partial { ref why, .. }
                if why.contains("2 listed conversations are not shown")
        ));
    }

    #[test]
    fn the_machine_scope_shows_a_conversation_whose_folder_was_never_approved() {
        // The row whose folder was never approved is exactly the row the operator could not see
        // before, and showing it is the whole point: every conversation on this machine in one
        // list, reachable before any window is moved (`memory/uxContract.md`). This surface is the
        // owner-only local endpoint, which the managed session index already opened for the same
        // reason; the phone speaks a different wire that carries no native discovery at all.
        let fixture = CatalogueFixture::make("machine");
        let machine = fixture.authorized(None);
        assert_eq!(
            machine.sessions.len(),
            2,
            "the unapproved folder's row survives"
        );
        assert!(
            machine
                .sessions
                .iter()
                .any(|session| session.cwd.as_str() == fixture.outside.as_str()),
            "and it is the one outside the approved root"
        );
        assert!(
            machine
                .sessions
                .iter()
                .any(|session| session.already_managed_as.is_some()),
            "deduplication and managed-session matching are unchanged by the wider scope"
        );
    }
}
