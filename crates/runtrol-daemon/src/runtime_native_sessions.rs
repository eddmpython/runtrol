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
    CatalogueCoverage, CatalogueSource, MAX_NATIVE_PUBLIC_CURSOR_BYTES, NATIVE_CURSOR_LIFETIME_MS,
    NativeResumeCapability, NativeSessionCatalogue, NativeSessionDescriptor, ProviderId,
};
use sha2::{Digest as _, Sha256};

use crate::runtime_auth::AuthorizedIntegration;
use crate::runtime_inventory::{AuthorizedRoot, RuntimeInventoryFailure, RuntimeSessionCatalogue};

const CURSOR_VERSION: u8 = 1;
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
        root: &AuthorizedRoot,
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
        body.extend_from_slice(&digest(root.path.as_str()));
        body.extend_from_slice(&root.identity);
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
        root: &AuthorizedRoot,
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
}

fn decode_body(
    body: &[u8],
    authority: &AuthorizedIntegration,
    provider: CoreProviderId,
    root: &AuthorizedRoot,
    binary_identity: [u8; 32],
) -> Result<OpenedCursor, NativeCursorFailure> {
    let mut cursor = Cursor { bytes: body, at: 0 };
    if cursor.byte()? != CURSOR_VERSION
        || cursor.fixed::<16>()? != authority.key.to_bytes()
        || cursor.u64()? != authority.grant.key_generation
        || cursor.u64()? != authority.grant.grant_generation
        || cursor.fixed::<32>()? != digest(provider.as_str())
        || cursor.fixed::<32>()? != digest(root.path.as_str())
        || cursor.fixed::<24>()? != root.identity
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
pub(crate) fn authorize_catalogue(
    authority: &AuthorizedIntegration,
    selected_root: &AuthorizedRoot,
    approved_roots: &[AuthorizedRoot],
    managed: &RuntimeSessionCatalogue,
    provider: CoreProviderId,
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
        if !cwd.is_under(&selected_root.path) {
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
        if additional
            .iter()
            .any(|path| !approved_roots.iter().any(|root| path.is_under(&root.path)))
        {
            omitted = omitted.saturating_add(1);
            continue;
        }
        let already_managed_as = managed.managed_as(authority, provider, &entry.native)?;
        sessions.push(NativeSessionDescriptor {
            native_session_id: entry.native.to_string(),
            cwd: cwd.to_string(),
            additional_directories: additional
                .into_iter()
                .map(|path| path.to_string())
                .collect(),
            title: entry.title.map(String::from),
            updated_at: entry.updated_at.map(String::from),
            resume: map_resume(entry.resume),
            already_managed_as,
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
            why: format!("{omitted} provider entries failed approved-root or duplicate filtering"),
        }),
        ProviderCoverage::Partial { source, why } => {
            if why.len() > MAX_REASON_BYTES || why.chars().any(char::is_control) {
                return Err(RuntimeInventoryFailure::Unavailable);
            }
            let why = if omitted == 0 {
                String::from(why)
            } else {
                format!(
                    "{why}; {omitted} provider entries failed approved-root or duplicate filtering"
                )
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
                &root,
                [4; 32],
                "provider-page-2",
                None,
            )
            .expect("sealed");
        let opened = codec
            .open(&authority, provider, &root, [4; 32], &token)
            .expect("same context opens");
        assert_eq!(opened.provider_cursor.as_ref(), "provider-page-2");
        assert!(
            codec
                .open(&authority, provider, &root, [5; 32], &token)
                .is_err()
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
                &root,
                [4; 32],
                "provider-page-2",
                None,
            )
            .expect("first cursor");
        let opened = codec
            .open(&authority, provider, &root, [4; 32], &first)
            .expect("open first");
        assert!(
            codec
                .seal(
                    &authority,
                    provider,
                    &root,
                    [4; 32],
                    "provider-page-2",
                    Some(&opened)
                )
                .is_err()
        );
    }

    #[test]
    fn catalogue_filters_roots_deduplicates_and_merges_by_native_identity() {
        let base =
            std::env::temp_dir().join(format!("runtrol-native-catalogue-{}", std::process::id()));
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
        let native = || NativeSessionId::new("native-1").expect("valid native");
        let public = authorize_catalogue(
            &authority,
            &root,
            std::slice::from_ref(&root),
            &managed,
            provider,
            ProviderCatalogue {
                coverage: NativeCatalogueCoverage::Complete {
                    source: NativeCatalogueSource::OfficialProtocol,
                },
                sessions: vec![
                    NativeSessionEntry {
                        native: native(),
                        cwd: project.as_str().into(),
                        additional_directories: Vec::new(),
                        title: Some("Provider title".into()),
                        updated_at: None,
                        resume: ProviderNativeResume::Available,
                    },
                    NativeSessionEntry {
                        native: native(),
                        cwd: project.as_str().into(),
                        additional_directories: Vec::new(),
                        title: None,
                        updated_at: None,
                        resume: ProviderNativeResume::Available,
                    },
                    NativeSessionEntry {
                        native: NativeSessionId::new("native-outside").expect("valid native"),
                        cwd: outside.as_str().into(),
                        additional_directories: Vec::new(),
                        title: None,
                        updated_at: None,
                        resume: ProviderNativeResume::Available,
                    },
                ],
                next_cursor: None,
            },
        )
        .expect("filter catalogue");
        assert_eq!(public.sessions.len(), 1);
        assert!(
            public
                .sessions
                .first()
                .is_some_and(|session| session.already_managed_as.is_some())
        );
        assert!(matches!(
            public.coverage,
            CatalogueCoverage::Partial { ref why, .. }
                if why.contains("2 provider entries")
        ));
        std::fs::remove_dir_all(base).expect("clean fixture");
    }
}
