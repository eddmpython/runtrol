//! 의존 방향 게이트. **아키텍처의 SSOT 는 이 파일의 `ALLOWED_EDGES` 표다.**
//!
//! `mainPlan/` 문서는 검토용 사본이고 권위가 아니다. 표와 문서가 갈라지면 표가 이긴다.
//!
//! 선언된 의존을 본다 (`cargo metadata --no-deps`). resolve 그래프를 쓰지 않는 이유:
//! 아키텍처 규칙은 *선언*에 대한 것이므로 `optional = true` 나 `cfg(...)` 뒤에 숨은 금지
//! edge 도 잡혀야 한다. resolve 그래프는 feature 조합에 따라 같은 위반을 통과시킨다.
//! 부수 효과로 레지스트리 접근이 없어 오프라인에서 1 초 안에 돈다.
//!
//! dev-dependency 는 제외한다. cargo 는 dev 경로의 순환을 허용하고 (테스트 하네스가 위를
//! 가리키는 것은 정상), `runtrol-audit` 자신이 모든 crate 를 dev 로 의존한다.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cargo_metadata::{DependencyKind, MetadataCommand, Package};

/// 허용된 직접 의존. **아키텍처의 정본.**
///
/// 빈 슬라이스는 잎(leaf) 을 뜻한다: 어떤 워크스페이스 crate 에도 의존하지 않는다.
const ALLOWED_EDGES: &[(&str, &[&str])] = &[
    // L0. 어휘. 제3자 provider 저작자가 의존하는 semver 안정 표면.
    ("runtrol-provider", &[]),
    // L1. 기법. 어휘만 안다.
    ("runtrol-security", &["runtrol-provider"]),
    ("runtrol-childproc", &["runtrol-provider"]),
    ("runtrol-store", &["runtrol-provider"]),
    ("runtrol-ipc", &["runtrol-provider"]),
    // L2. 커널. **drivers 를 보지 못한다** (아래 FORBIDDEN 참조).
    (
        "runtrol-core",
        &[
            "runtrol-provider",
            "runtrol-security",
            "runtrol-childproc",
            "runtrol-store",
        ],
    ),
    // L2. 내장 드라이버. **store 를 보지 못한다** (얇음을 의존 edge 로 표현한 것).
    (
        "runtrol-drivers",
        &["runtrol-provider", "runtrol-childproc"],
    ),
    // L3. 조립.
    (
        "runtrol-daemon",
        &[
            "runtrol-provider",
            "runtrol-security",
            "runtrol-childproc",
            "runtrol-store",
            "runtrol-ipc",
            "runtrol-core",
            "runtrol-drivers",
        ],
    ),
    // L3. CLI 는 데몬에 묻는다. 저장소를 직접 열지 않는다 (배타적 락이라 애초에 불가능하다).
    ("runtrol-cli", &["runtrol-provider", "runtrol-ipc"]),
    // L4. 얇은 bin. 모든 것을 링크하는 유일한 예외를 여기 가둔다.
    ("runtrol", &["runtrol-cli", "runtrol-daemon"]),
    // 게이트 crate. 제품 의존은 없다 (전부 dev-dependency 라 이 표에 안 나타난다).
    ("runtrol-audit", &[]),
];

/// 전이적으로도 금지된 쌍. `ALLOWED_EDGES` 가 함의하지만, 실패 메시지가 단일 edge 가 아니라
/// **아키텍처 규칙의 이름**을 말하게 하려고 따로 적는다.
const FORBIDDEN_TRANSITIVE: &[(&str, &str, &str)] = &[
    (
        "runtrol-core",
        "runtrol-drivers",
        "provider 추가가 코어를 건드리지 않는다는 규칙. 코어는 트레이트를 정의하고 드라이버가 값을 공급하며 조립은 데몬이 한다",
    ),
    (
        "runtrol-drivers",
        "runtrol-store",
        "드라이버는 아무것도 저장하지 않는다. 저장소에 닿을 수 없는 드라이버는 transcript 를 갖기 시작할 수 없다",
    ),
    (
        "runtrol-security",
        "runtrol-core",
        "스코프 벽은 잎이어야 한다. 커널 내부 사정으로 벽을 약화시킬 수 없고, 미래의 원격 전송이 커널 없이 벽만 의존할 수 있다",
    ),
    (
        "runtrol-cli",
        "runtrol-store",
        "CLI 는 데몬에 묻는다. 데몬이 DB 를 들고 있으면 두 번째 opener 는 배타적 락에 막힌다 (실측)",
    ),
    (
        "runtrol-cli",
        "runtrol-core",
        "CLI 는 세션을 직접 감독하지 않는다",
    ),
];

/// crate 이름 -> 워크스페이스 내부 production 의존 이름 집합.
fn workspace_graph() -> BTreeMap<String, BTreeSet<String>> {
    let metadata = MetadataCommand::new()
        // 이 crate 의 매니페스트를 가리키면 cargo 가 소속 워크스페이스 전체를 보고한다.
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        // 선언된 edge 만. 오프라인이고 빠르며, `metadata.packages` 가 멤버로 좁혀진다.
        .no_deps()
        .exec()
        .unwrap_or_else(|error| panic!("`cargo metadata` 실패: {error}"));

    let members: Vec<&Package> = metadata.workspace_packages();
    // `Package::name` 은 0.20 부터 `PackageName` newtype 이다. `String` 이 아니다.
    let member_names: BTreeSet<&str> = members.iter().map(|p| p.name.as_str()).collect();

    let mut graph: BTreeMap<String, BTreeSet<String>> = member_names
        .iter()
        .map(|name| ((*name).to_owned(), BTreeSet::new()))
        .collect();

    for package in &members {
        for dependency in &package.dependencies {
            // production edge 만. dev 는 순환이 합법이고 게이트 crate 가 전부를 dev 로 의존한다.
            match dependency.kind {
                DependencyKind::Normal | DependencyKind::Build => {}
                _ => continue,
            }
            // `Dependency::name` 은 rename 과 무관하게 실제 crate 이름이다.
            if !member_names.contains(dependency.name.as_str()) {
                continue;
            }
            // 같은 이름의 crates.io crate 와 구분한다. 내부 edge 는 path 의존이다.
            if dependency.path.is_none() {
                continue;
            }
            let entry = graph
                .get_mut(package.name.as_str())
                .unwrap_or_else(|| panic!("멤버 {} 항목이 없다", package.name));
            entry.insert(dependency.name.clone());
        }
    }
    graph
}

/// `start` 에서 도달 가능한 전부. 순환이 되돌아오면 `start` 자신도 포함된다.
fn reachable<'g>(graph: &'g BTreeMap<String, BTreeSet<String>>, start: &str) -> BTreeSet<&'g str> {
    let mut seen: BTreeSet<&'g str> = BTreeSet::new();
    let mut queue: VecDeque<&'g str> = VecDeque::new();
    for direct in graph.get(start).into_iter().flatten() {
        if seen.insert(direct.as_str()) {
            queue.push_back(direct.as_str());
        }
    }
    while let Some(node) = queue.pop_front() {
        for next in graph.get(node).into_iter().flatten() {
            if seen.insert(next.as_str()) {
                queue.push_back(next.as_str());
            }
        }
    }
    seen
}

/// 실패 메시지에 넣을 최단 경로. `from == to` 도 되므로 순환 보고에 쓴다.
fn shortest_path(
    graph: &BTreeMap<String, BTreeSet<String>>,
    from: &str,
    to: &str,
) -> Option<String> {
    let start = graph.get_key_value(from)?.0.as_str();
    let mut previous: BTreeMap<&str, &str> = BTreeMap::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::from([start]);
    // start 를 미리 seen 에 넣지 않는다. 그래야 자기 자신으로의 순환도 발견된다.
    while let Some(node) = queue.pop_front() {
        for next in graph.get(node).into_iter().flatten() {
            let next = next.as_str();
            if !seen.insert(next) {
                continue;
            }
            previous.insert(next, node);
            if next == to {
                let mut chain = vec![next];
                let mut cursor = next;
                while let Some(&parent) = previous.get(cursor) {
                    chain.push(parent);
                    if parent == start {
                        break;
                    }
                    cursor = parent;
                }
                chain.reverse();
                return Some(chain.join(" -> "));
            }
            queue.push_back(next);
        }
    }
    None
}

#[test]
fn only_declared_edges_exist() {
    let graph = workspace_graph();
    let allowed: BTreeMap<&str, BTreeSet<&str>> = ALLOWED_EDGES
        .iter()
        .map(|(from, tos)| (*from, tos.iter().copied().collect()))
        .collect();

    let mut violations: Vec<String> = Vec::new();

    // 실재하는 모든 edge 가 표에 있어야 한다.
    for (from, tos) in &graph {
        match allowed.get(from.as_str()) {
            None => violations.push(format!(
                "워크스페이스 멤버 `{from}` 가 ALLOWED_EDGES 에 없다. \
                 tests/audit/dependencyDirection.rs 에 추가하고 계층을 정하라"
            )),
            Some(allowed_tos) => {
                for to in tos {
                    if !allowed_tos.contains(to.as_str()) {
                        violations.push(format!(
                            "금지된 의존 `{from}` -> `{to}`: ALLOWED_EDGES 에 없다"
                        ));
                    }
                }
            }
        }
    }

    // 그리고 표가 낡지 않았어야 한다 (없는 멤버를 가리키면 그것도 회귀다).
    for (from, tos) in ALLOWED_EDGES {
        if !graph.contains_key(*from) {
            violations.push(format!(
                "ALLOWED_EDGES 가 `{from}` 를 적었으나 워크스페이스 멤버가 아니다"
            ));
            continue;
        }
        for to in *tos {
            if !graph.contains_key(*to) {
                violations.push(format!(
                    "ALLOWED_EDGES 가 `{from}` -> `{to}` 를 허용하나 `{to}` 는 멤버가 아니다"
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "워크스페이스 아키텍처 위반:\n  - {}",
        violations.join("\n  - ")
    );
}

#[test]
fn forbidden_pairs_are_unreachable() {
    let graph = workspace_graph();
    let mut violations: Vec<String> = Vec::new();

    for (from, to, rule) in FORBIDDEN_TRANSITIVE {
        assert!(
            graph.contains_key(*from),
            "`{from}` 는 워크스페이스 멤버가 아니다"
        );
        assert!(
            graph.contains_key(*to),
            "`{to}` 는 워크스페이스 멤버가 아니다"
        );
        if reachable(&graph, from).contains(to) {
            let path = shortest_path(&graph, from, to).unwrap_or_else(|| "<불명>".to_owned());
            violations.push(format!("`{from}` -> `{to}` ({rule}). 경로: {path}"));
        }
    }

    assert!(
        violations.is_empty(),
        "계층 역전:\n  - {}",
        violations.join("\n  - ")
    );
}

#[test]
fn no_production_cycles() {
    let graph = workspace_graph();
    let mut violations: Vec<String> = Vec::new();
    for member in graph.keys() {
        if reachable(&graph, member).contains(member.as_str()) {
            violations
                .push(shortest_path(&graph, member, member).unwrap_or_else(|| member.clone()));
        }
    }
    assert!(
        violations.is_empty(),
        "production 의존 순환:\n  - {}",
        violations.join("\n  - ")
    );
}
