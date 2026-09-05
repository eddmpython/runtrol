use super::*;
use crate::isolated_workspace::VerifiedProject;
use crate::isolated_workspace::ownership::SpawnTicket;
use crate::isolated_workspace::tests::Scratch;
use crate::terminal_surface::{HostedTerminal, TerminalOrigin};
use runtrol_core::terminal::Terminal;
use runtrol_runtime_protocol::{AppScope, IntegrationGrant, IntegrationId};
use runtrol_store::{IntegrationKey, IntegrationRootRow, IntegrationRow};

struct Fixture {
    composed: Arc<Composed>,
    authority: AuthorizedIntegration,
    row: IntegrationRow,
    workspace: AbsPath,
    scratch: Scratch,
}

impl Fixture {
    async fn new() -> Self {
        let scratch = Scratch::make();
        let home = scratch.root.join("runtime");
        std::fs::create_dir(&home).unwrap();
        let composed = Arc::new(
            Composed::for_tests(home.to_str().unwrap(), runtrol_drivers::builtin()).unwrap(),
        );
        let project = VerifiedProject::discover(&scratch.project).unwrap();
        let process = runtrol_childproc::process_identity(std::process::id()).unwrap();
        let ticket = SpawnTicket::new(process, TerminalId::now(), TerminalId::now(), 1).unwrap();
        let workspace = {
            let mut controller = composed.isolated_workspaces.lock().await;
            let prepared = controller
                .prepare_terminal(&composed.containment, &ticket, &project)
                .await
                .unwrap();
            controller
                .bind_terminal(&ticket, process, &prepared.workspace)
                .unwrap();
            prepared.workspace
        };
        let root = IntegrationRootRow {
            path: scratch.project.to_string().into(),
            identity: project.root_identity(),
        };
        let row = IntegrationRow {
            public_key: [3; 32],
            client_instance_id: "owned-resume-fixture".into(),
            label: "Owned resume".into(),
            manifest_digest: [4; 32],
            scopes: vec!["session.resume".into(), "session.input.write".into()],
            roots: vec![root.clone()],
            key_generation: 1,
            grant_generation: 1,
            approved_at: WallMs::now(),
            revoked_at: None,
        };
        let key = IntegrationKey::from_bytes([6; 16]);
        composed
            .integration_authority
            .publish_committed(key, row.clone())
            .unwrap();
        let authority = AuthorizedIntegration {
            key,
            grant: IntegrationGrant {
                integration_id: IntegrationId::new("owned-resume"),
                scopes: vec![AppScope::SessionResume, AppScope::SessionInputWrite],
                roots: vec![scratch.project.to_string()],
                key_generation: 1,
                grant_generation: 1,
            },
            roots: vec![root],
        };
        Self {
            composed,
            authority,
            row,
            workspace,
            scratch,
        }
    }

    fn native() -> TerminalOpenTarget {
        TerminalOpenTarget::Native {
            native_session_id: "fixture-native".to_owned(),
            adoption_token: "fixture-observation".to_owned(),
        }
    }

    async fn launch(&self) -> ResumeLaunch {
        prepare(
            &self.composed,
            &self.authority,
            &self.workspace,
            &Self::native(),
        )
        .await
        .unwrap()
        .unwrap()
    }
}

#[tokio::test]
async fn original_resume_only_grant_keeps_project_visibility_without_fabricated_lineage() {
    let fixture = Fixture::new().await;
    assert!(
        !fixture
            .authority
            .grant
            .scopes
            .contains(&AppScope::SessionStart)
    );
    let launch = fixture.launch().await;
    launch.validate(&fixture.composed).unwrap();
    // Inspection must not reject the original live occupant before the existing native join can run.
    assert!(
        fixture
            .composed
            .isolated_workspaces
            .lock()
            .await
            .resume_binding(&fixture.workspace)
            .unwrap()
            .is_some()
    );
    let terminal = Terminal::fed(0, runtrol_childproc::PtySize { cols: 80, rows: 24 }).unwrap();
    let hosted = HostedTerminal {
        spawned: None,
        resumed: Some(Arc::clone(&launch.owned)),
        id: launch.owned.owner.terminal,
        provider: CoreProviderId::parse("fixture").unwrap(),
        terminal: terminal.clone(),
        workspace: fixture.workspace.clone(),
        native: Some("fixture-native".into()),
        opened_at_ms: WallMs::now().as_millis(),
        generation: 1,
        stopping: false,
        origin: TerminalOrigin::Owned,
    };
    assert_eq!(hosted.project_root(), &fixture.scratch.project);
    super::super::ensure_visible(&hosted, &fixture.authority).unwrap();
    let (changes, _) = tokio::sync::watch::channel(0);
    let descriptor = super::super::descriptor(
        &hosted,
        "fixture",
        &changes,
        super::super::ControlView::default(),
        false,
    )
    .unwrap();
    assert_eq!(
        descriptor.project_root.as_deref(),
        Some(fixture.scratch.project.as_str())
    );
    assert!(descriptor.spawned_by.is_none());
    assert!(descriptor.initial_message_id.is_none());
    #[cfg(windows)]
    {
        let pinned = super::super::pin_visible_root(&fixture.authority, &hosted).unwrap();
        assert!(pinned.guard.lock().await.valid());
    }
    terminal.end_feed(None).unwrap();
}

#[tokio::test]
async fn a_worktree_grant_still_requires_occupancy_and_cannot_turn_resume_into_fresh() {
    let mut fixture = Fixture::new().await;
    let root = IntegrationRootRow {
        path: fixture.workspace.to_string().into(),
        identity: runtrol_security::ProjectRootIdentity::read(&fixture.workspace)
            .unwrap()
            .to_bytes(),
    };
    fixture.row.roots.push(root.clone());
    fixture.row.grant_generation += 1;
    fixture.authority.grant.grant_generation = fixture.row.grant_generation;
    fixture.authority.roots.push(root);
    fixture
        .authority
        .grant
        .roots
        .push(fixture.workspace.to_string());
    fixture
        .composed
        .integration_authority
        .publish_committed(fixture.authority.key, fixture.row.clone())
        .unwrap();
    assert!(
        prepare(
            &fixture.composed,
            &fixture.authority,
            &fixture.workspace,
            &Fixture::native()
        )
        .await
        .unwrap()
        .is_some()
    );
    assert!(
        prepare(
            &fixture.composed,
            &fixture.authority,
            &fixture.workspace,
            &TerminalOpenTarget::Fresh
        )
        .await
        .is_err()
    );
    assert!(
        prepare(
            &fixture.composed,
            &fixture.authority,
            &fixture.scratch.project,
            &TerminalOpenTarget::Fresh
        )
        .await
        .unwrap()
        .is_none()
    );
}

#[tokio::test]
async fn another_project_or_late_scope_revocation_cannot_authorize_the_binding() {
    let mut fixture = Fixture::new().await;
    let launch = fixture.launch().await;
    fixture.row.scopes.clear();
    fixture.row.grant_generation += 1;
    fixture
        .composed
        .integration_authority
        .publish_committed(fixture.authority.key, fixture.row.clone())
        .unwrap();
    assert!(launch.validate(&fixture.composed).is_err());
    fixture.row.scopes = vec!["session.resume".into()];
    fixture.row.grant_generation += 1;
    let foreign = Scratch::make();
    fixture.row.roots = vec![IntegrationRootRow {
        path: foreign.project.to_string().into(),
        identity: runtrol_security::ProjectRootIdentity::read(&foreign.project)
            .unwrap()
            .to_bytes(),
    }];
    fixture
        .composed
        .integration_authority
        .publish_committed(fixture.authority.key, fixture.row.clone())
        .unwrap();
    assert!(
        prepare(
            &fixture.composed,
            &fixture.authority,
            &fixture.workspace,
            &Fixture::native()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn a_changed_approval_cannot_refresh_an_already_checked_native_observation() {
    let mut fixture = Fixture::new().await;
    let launch = fixture.launch().await;
    fixture.row.grant_generation += 1;
    fixture
        .composed
        .integration_authority
        .publish_committed(fixture.authority.key, fixture.row.clone())
        .unwrap();
    assert!(launch.validate(&fixture.composed).is_err());
    fixture.authority.grant.grant_generation = fixture.row.grant_generation;
    fixture.launch().await.validate(&fixture.composed).unwrap();
}

async fn reject_approved_owned_descendant(target: TerminalOpenTarget) {
    let mut fixture = Fixture::new().await;
    let root = IntegrationRootRow {
        path: fixture.workspace.to_string().into(),
        identity: runtrol_security::ProjectRootIdentity::read(&fixture.workspace)
            .unwrap()
            .to_bytes(),
    };
    fixture.row.roots.push(root.clone());
    fixture.row.grant_generation += 1;
    fixture.authority.roots.push(root);
    fixture
        .authority
        .grant
        .roots
        .push(fixture.workspace.to_string());
    fixture.authority.grant.grant_generation = fixture.row.grant_generation;
    fixture
        .composed
        .integration_authority
        .publish_committed(fixture.authority.key, fixture.row.clone())
        .unwrap();
    let child = fixture.workspace.join("nested").unwrap();
    std::fs::create_dir(child.as_std_path()).unwrap();
    let child = AbsPath::canonicalize(child.as_str()).unwrap();
    let ordinary = fixture.scratch.project.join("nested").unwrap();
    std::fs::create_dir(ordinary.as_std_path()).unwrap();
    assert!(
        prepare(&fixture.composed, &fixture.authority, &ordinary, &target)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        prepare(&fixture.composed, &fixture.authority, &child, &target)
            .await
            .is_err(),
        "an approved child cwd cannot bypass the Core worktree occupant"
    );
}

#[tokio::test]
async fn an_approved_owned_descendant_cannot_launch_fresh_without_occupancy() {
    reject_approved_owned_descendant(TerminalOpenTarget::Fresh).await;
}

#[tokio::test]
async fn an_approved_owned_descendant_cannot_change_the_recorded_native_cwd() {
    reject_approved_owned_descendant(Fixture::native()).await;
}

#[tokio::test]
async fn worktree_observations_complete_while_an_unrelated_controller_operation_is_held() {
    let fixture = Fixture::new().await;
    let controller = fixture.composed.isolated_workspaces.lock().await;
    let composed = Arc::clone(&fixture.composed);
    let authority = fixture.authority.clone();
    let ordinary = fixture.scratch.project.clone();
    let workspace = fixture.workspace.clone();
    let mut observations = tokio::spawn(async move {
        tokio::join!(
            async {
                let ordinary_allowed =
                    crate::isolated_workspace::refuse_unbound_worktree(&composed, &ordinary)
                        .await
                        .is_ok();
                let owned_refused =
                    crate::isolated_workspace::refuse_unbound_worktree(&composed, &workspace)
                        .await
                        .is_err();
                (ordinary_allowed, owned_refused)
            },
            async {
                let ordinary_unbound =
                    prepare(&composed, &authority, &ordinary, &TerminalOpenTarget::Fresh)
                        .await
                        .is_ok_and(|binding| binding.is_none());
                let fresh_refused = prepare(
                    &composed,
                    &authority,
                    &workspace,
                    &TerminalOpenTarget::Fresh,
                )
                .await
                .is_err();
                let resume_ready = prepare(&composed, &authority, &workspace, &Fixture::native())
                    .await
                    .is_ok_and(|binding| binding.is_some());
                (ordinary_unbound, fresh_refused, resume_ready)
            }
        )
    });
    let before_release =
        tokio::time::timeout(std::time::Duration::from_secs(2), &mut observations).await;
    let completed_while_held = before_release.is_ok();
    drop(controller);
    // Finish even the red-path reader before the fixture removes its owned files.
    let observed = match before_release {
        Ok(result) => result.unwrap(),
        Err(_) => tokio::time::timeout(std::time::Duration::from_secs(5), observations)
            .await
            .unwrap()
            .unwrap(),
    };
    assert!(
        completed_while_held,
        "read-only observations must finish before the controller operation is released"
    );
    assert_eq!(observed, ((true, true), (true, true, true)));
}
