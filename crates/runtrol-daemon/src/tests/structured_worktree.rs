use super::*;
use crate::isolated_workspace::VerifiedProject;
use crate::isolated_workspace::ownership::{EndedSpawn, SpawnTicket};
use crate::isolated_workspace::tests::{Scratch, process::ProcessScratch};

pub(crate) struct Fixture {
    pub(crate) composed: Arc<Composed>,
    pub(crate) owned: AbsPath,
    pub(crate) structured: AbsPath,
    pub(crate) scratch: Scratch,
}

impl Fixture {
    pub(crate) async fn new() -> Self {
        let scratch = Scratch::make();
        let home = scratch.root.join("runtime");
        std::fs::create_dir(&home).unwrap();
        let composed = Arc::new(
            Composed::for_tests(home.to_str().unwrap(), runtrol_drivers::builtin()).unwrap(),
        );
        let process = runtrol_childproc::process_identity(std::process::id()).unwrap();
        let ticket = SpawnTicket::new(
            process,
            runtrol_provider::TerminalId::now(),
            runtrol_provider::TerminalId::now(),
            1,
        )
        .unwrap();
        let project = VerifiedProject::discover(&scratch.project).unwrap();
        let (owned, structured) = {
            let mut controller = composed.isolated_workspaces.lock().await;
            let prepared = controller
                .prepare_terminal(&composed.containment, &ticket, &project)
                .await
                .unwrap();
            let mut worker = ProcessScratch::start(&scratch);
            controller
                .bind_terminal(&ticket, worker.identity, &prepared.workspace)
                .unwrap();
            worker.stop();
            std::fs::write(prepared.workspace.as_std_path().join("kept.txt"), b"kept\n").unwrap();
            controller
                .release_terminal_if_present(
                    &composed.containment,
                    &EndedSpawn::after_gate_retired(ticket),
                )
                .await
                .unwrap()
                .expect("the dirty ended terminal worktree is retained");
            let Response::IsolatedWorkspace(structured) = controller
                .prepare(
                    &composed.containment,
                    &uuid::Uuid::now_v7().to_string(),
                    scratch.project.as_str(),
                )
                .await
                .unwrap()
            else {
                panic!("the existing structured isolation path prepares a worktree");
            };
            let structured = AbsPath::canonicalize(&structured.workspace).unwrap();
            (prepared.workspace, structured)
        };
        Self {
            composed,
            owned,
            structured,
            scratch,
        }
    }

    pub(crate) fn child(workspace: &AbsPath) -> AbsPath {
        let child = workspace.join("child").unwrap();
        std::fs::create_dir_all(child.as_std_path()).unwrap();
        child
    }
}

async fn legacy_requests(fixture: &Fixture, paths: &[AbsPath]) -> (Vec<Response>, usize) {
    let composed = Arc::clone(&fixture.composed);
    let mut conversation = Conversation::at_the_machine();
    assert!(matches!(
        answer_prepared(
            &mut conversation,
            &composed,
            &mut SessionManager::new(),
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
            Prepared::None,
            None,
        )
        .await,
        Reply::One(Response::Welcome { .. })
    ));
    let address = composed.home.paths().endpoint().address().to_owned();
    let mut listener = Listener::bind(&address).await.unwrap();
    let (server, caller) =
        tokio::join!(listener.accept(), runtrol_ipc::transport::connect(&address));
    let mut caller = caller.unwrap();
    let (asking, mut asked) = mpsc::channel(1);
    let (reserving, mut reservations) = mpsc::unbounded_channel();
    let (returning, mut returned) = mpsc::unbounded_channel();
    let (_, session_index) = watch::channel(PublishedIndex {
        full_frame: Arc::from([]),
        listing: None,
    });
    let services = ConnectionServices {
        asking,
        reserving,
        returning,
        discovering: Arc::new(DiscoveryGates::new(&composed.registry)),
        composed,
        session_index,
    };
    let serving = tokio::spawn(converse(
        SurfaceConnection::Local(server.unwrap()),
        conversation,
        services,
    ));
    // Every admitted path stops at the owner channel, before discovery or any provider call.
    let owner = tokio::spawn(async move {
        let mut count = 0;
        while let Some(request) = reservations.recv().await {
            let ReservationAsked::Reserve { answered, .. } = request else {
                panic!("only a new structured reservation belongs to this fixture");
            };
            count += 1;
            answered
                .send(Err(SessionError::OpeningCapacityReserved.into()))
                .unwrap_or_else(|_| panic!("the opening caller still owns its reply"));
        }
        count
    });
    let mut responses = Vec::new();
    for path in paths {
        for request in [
            Request::Start {
                provider: "fixture".into(),
                workspace: path.as_str().into(),
                workspace_access: WorkspaceAccess::Exclusive,
                model: None,
                permission: None,
            },
            Request::Resume {
                provider: "fixture".into(),
                native: "different-native".into(),
                workspace: path.as_str().into(),
                workspace_access: WorkspaceAccess::Exclusive,
            },
        ] {
            caller
                .send(&serde_json::to_vec(&request).unwrap())
                .await
                .unwrap();
            let frame = caller.recv().await.unwrap().unwrap();
            responses.push(serde_json::from_slice(&frame).unwrap());
        }
    }
    drop(caller);
    serving.await.unwrap();
    let count = owner.await.unwrap();
    assert!(
        asked.try_recv().is_err(),
        "no provider preparation or dispatch"
    );
    assert!(
        returned.try_recv().is_err(),
        "no provider process was opened"
    );
    (responses, count)
}

#[tokio::test]
async fn legacy_structured_open_refuses_retained_terminal_worktrees_before_reservation() {
    let fixture = Fixture::new().await;
    let (responses, count) = legacy_requests(
        &fixture,
        &[fixture.owned.clone(), Fixture::child(&fixture.owned)],
    )
    .await;
    assert_eq!(
        count, 0,
        "retained worktrees must never reach the session owner"
    );
    for response in responses {
        let Response::Failed(error) = response else {
            panic!("the legacy wire must retain its existing refusal envelope");
        };
        assert!(error.message.contains("Core-owned worktree"));
    }
    assert!(fixture.owned.as_std_path().join("kept.txt").exists());
}

#[tokio::test]
async fn legacy_structured_open_keeps_original_and_session_owned_workspaces() {
    let fixture = Fixture::new().await;
    let paths = [
        fixture.scratch.project.clone(),
        Fixture::child(&fixture.scratch.project),
        fixture.structured.clone(),
        Fixture::child(&fixture.structured),
    ];
    let (responses, count) = legacy_requests(&fixture, &paths).await;
    assert_eq!(count, paths.len() * 2);
    for response in responses {
        let Response::Failed(error) = response else {
            panic!("the fixture owner refuses before provider preparation");
        };
        assert_eq!(
            error.message.as_ref(),
            SessionError::OpeningCapacityReserved.to_string()
        );
    }
}
