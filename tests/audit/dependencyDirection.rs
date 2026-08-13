//! The dependency-direction gate. **The architecture's single source of truth is the `ALLOWED_EDGES` table
//! in this file.**
//!
//! The documents under `mainPlan/` are a copy for review and carry no authority. Where the table and a document
//! disagree, the table wins.
//!
//! It reads declared dependencies (`cargo metadata --no-deps`) rather than the resolved graph. An architectural
//! rule is about what a crate *declares*, so a forbidden edge hidden behind `optional = true` or a `cfg(...)`
//! has to be caught; the resolved graph lets the same violation through depending on which features are on. A
//! useful side effect is that it touches no registry, so it runs offline in under a second.
//!
//! Development dependencies are excluded. Cargo allows cycles along that path (a test harness pointing upwards
//! is normal), and `runtrol-audit` itself depends on every crate that way.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cargo_metadata::{DependencyKind, MetadataCommand, Package};

/// Every direct dependency that is allowed. **The architecture itself.**
///
/// An empty slice means a leaf: it depends on no workspace crate at all.
const ALLOWED_EDGES: &[(&str, &[&str])] = &[
    // Public Runtime vocabulary. It is provider-neutral and imports no private control or Core type.
    ("runtrol-runtime-protocol", &[]),
    // Public consumer SDK. Its only workspace dependency is the public wire contract.
    ("runtrol-runtime-client", &["runtrol-runtime-protocol"]),
    // Mission evidence accepts provider identities but no conversation-capable events or control layers.
    ("runtrol-ledger", &["runtrol-provider"]),
    (
        "runtrol-orchestrator",
        &[
            "runtrol-provider",
            "runtrol-security",
            "runtrol-core",
            "runtrol-ledger",
        ],
    ),
    // L0. The vocabulary. The semver-stable surface a third-party provider author depends on.
    ("runtrol-provider", &[]),
    // L1. The techniques. Each knows the vocabulary and nothing else.
    ("runtrol-security", &["runtrol-provider"]),
    ("runtrol-childproc", &["runtrol-provider"]),
    ("runtrol-store", &["runtrol-provider"]),
    ("runtrol-ipc", &["runtrol-provider"]),
    // L1. Browser-reachable transport. It may establish a remote caller through the scope wall, but cannot
    // reach the kernel, a driver, storage, or the local presence challenge.
    ("runtrol-transport", &["runtrol-security"]),
    // L1. Per-user machine secret protection. It knows only the path vocabulary and never transport, storage, or a
    // conversation-capable type.
    ("runtrol-vault", &["runtrol-provider"]),
    // L1. Signed release and filesystem replacement primitives. It opens no socket and starts no provider.
    ("runtrol-update", &[]),
    // L2. The kernel. **It cannot see the drivers** (see FORBIDDEN_TRANSITIVE below).
    (
        "runtrol-core",
        &[
            "runtrol-provider",
            "runtrol-security",
            "runtrol-childproc",
            "runtrol-store",
        ],
    ),
    // L2. The built-in drivers. **They cannot see storage**, which is the thin principle as a dependency edge.
    (
        "runtrol-drivers",
        &["runtrol-provider", "runtrol-childproc"],
    ),
    // L3. Assembly.
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
            "runtrol-transport",
            "runtrol-vault",
            "runtrol-update",
            "runtrol-runtime-protocol",
            "runtrol-ledger",
        ],
    ),
    // L3. The command surface asks the daemon. It never opens storage itself, which the exclusive lock
    // would refuse anyway.
    ("runtrol-cli", &["runtrol-provider", "runtrol-ipc"]),
    // L4. The thin binary. It links everything, which is the one exception to all of the above, and
    // confining that exception to one short file is what lets this table be strict about everything else. The
    // edges past the two personalities are what being the program requires: naming which providers this build
    // can drive, binding the endpoint when it is the daemon, and deciding once what this process passes on to
    // anything it starts.
    (
        "runtrol",
        &[
            "runtrol-cli",
            "runtrol-daemon",
            "runtrol-drivers",
            "runtrol-ipc",
            "runtrol-childproc",
        ],
    ),
    // The gate crate. No production dependencies at all: every one of them is a development dependency, so
    // none of them appears in this table.
    ("runtrol-audit", &[]),
];

/// Pairs that must not be reachable even indirectly.
///
/// `ALLOWED_EDGES` already implies every one of these. They are written out again so that a failure names **the
/// rule that was broken** rather than one edge in a chain, which is the difference between a message somebody
/// can act on and one they have to reconstruct.
const FORBIDDEN_TRANSITIVE: &[(&str, &str, &str)] = &[
    (
        "runtrol-core",
        "runtrol-drivers",
        "adding a provider does not touch the kernel. the kernel defines the traits, a driver supplies the values, and the daemon puts them together",
    ),
    (
        "runtrol-drivers",
        "runtrol-store",
        "a driver stores nothing. one that cannot reach storage cannot start keeping a copy of a conversation",
    ),
    (
        "runtrol-security",
        "runtrol-core",
        "the scope wall has to be a leaf. nothing inside the kernel can weaken it, and a future remote transport can depend on the wall without depending on the kernel",
    ),
    (
        "runtrol-transport",
        "runtrol-core",
        "a remote frame boundary establishes a device caller but cannot supervise sessions or mint local presence",
    ),
    (
        "runtrol-transport",
        "runtrol-drivers",
        "a remote transport carries provider-independent frames and cannot know a provider implementation",
    ),
    (
        "runtrol-transport",
        "runtrol-store",
        "a remote transport stores neither sessions nor a copy of conversation data",
    ),
    (
        "runtrol-cli",
        "runtrol-store",
        "the command surface asks the daemon. with the daemon holding the database, a second opener is refused by the exclusive lock (measured)",
    ),
    (
        "runtrol-cli",
        "runtrol-core",
        "the command surface does not supervise a session itself",
    ),
    (
        "runtrol-runtime-client",
        "runtrol-core",
        "the public SDK speaks only the public protocol and cannot supervise sessions or import private Core types",
    ),
    (
        "runtrol-ledger",
        "runtrol-drivers",
        "Mission evidence cannot discover or interpret a provider implementation",
    ),
    (
        "runtrol-orchestrator",
        "runtrol-drivers",
        "the Mission kernel emits provider-neutral effects and cannot call a provider implementation",
    ),
    (
        "runtrol-orchestrator",
        "runtrol-ipc",
        "the Mission kernel is independent from private and remote transports",
    ),
    (
        "runtrol-ledger",
        "runtrol-store",
        "Mission evidence owns a separate bounded file and cannot enter the session store",
    ),
    (
        "runtrol-runtime-client",
        "runtrol-ipc",
        "the public SDK owns its public framing and cannot import the private control transport",
    ),
];

/// Every workspace crate, and the workspace crates it declares as a production dependency.
fn workspace_graph() -> BTreeMap<String, BTreeSet<String>> {
    let metadata = MetadataCommand::new()
        // Pointing at this crate's own manifest makes cargo report the whole workspace it belongs to.
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        // Declared edges only. Offline, fast, and it narrows `metadata.packages` to the members.
        .no_deps()
        .exec()
        .unwrap_or_else(|error| panic!("`cargo metadata` failed: {error}"));

    let members: Vec<&Package> = metadata.workspace_packages();
    // Since 0.20 `Package::name` is a `PackageName` newtype rather than a `String`.
    let member_names: BTreeSet<&str> = members.iter().map(|p| p.name.as_str()).collect();

    let mut graph: BTreeMap<String, BTreeSet<String>> = member_names
        .iter()
        .map(|name| ((*name).to_owned(), BTreeSet::new()))
        .collect();

    for package in &members {
        for dependency in &package.dependencies {
            // Production edges only. Cycles are legal along the development path, and the gate crate
            // depends on everything that way.
            match dependency.kind {
                DependencyKind::Normal | DependencyKind::Build => {}
                _ => continue,
            }
            // `Dependency::name` is the real crate name whatever it was renamed to.
            if !member_names.contains(dependency.name.as_str()) {
                continue;
            }
            // Tells an internal edge apart from a crates.io crate of the same name: an internal one is a
            // path dependency.
            if dependency.path.is_none() {
                continue;
            }
            let entry = graph
                .get_mut(package.name.as_str())
                .unwrap_or_else(|| panic!("no entry for member {}", package.name));
            entry.insert(dependency.name.clone());
        }
    }
    graph
}

/// Everything reachable from `start`, including `start` itself when a cycle leads back to it.
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

/// The shortest path, for a failure message to name. `from == to` is allowed, which is how a cycle is
/// reported.
fn shortest_path(
    graph: &BTreeMap<String, BTreeSet<String>>,
    from: &str,
    to: &str,
) -> Option<String> {
    let start = graph.get_key_value(from)?.0.as_str();
    let mut previous: BTreeMap<&str, &str> = BTreeMap::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::from([start]);
    // `start` is deliberately not marked as seen up front, so that a cycle back to it is found.
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

    // Every edge that exists has to be in the table.
    for (from, tos) in &graph {
        match allowed.get(from.as_str()) {
            None => violations.push(format!(
                "workspace member `{from}` is not in ALLOWED_EDGES. \
                 add it to tests/audit/dependencyDirection.rs and decide which layer it belongs to"
            )),
            Some(allowed_tos) => {
                for to in tos {
                    if !allowed_tos.contains(to.as_str()) {
                        violations.push(format!(
                            "dependency `{from}` -> `{to}` is not allowed: it is not in ALLOWED_EDGES"
                        ));
                    }
                }
            }
        }
    }

    // And the table must not have gone stale: naming a member that does not exist is a regression too.
    for (from, tos) in ALLOWED_EDGES {
        if !graph.contains_key(*from) {
            violations.push(format!(
                "ALLOWED_EDGES names `{from}`, which is not a workspace member"
            ));
            continue;
        }
        for to in *tos {
            if !graph.contains_key(*to) {
                violations.push(format!(
                    "ALLOWED_EDGES allows `{from}` -> `{to}`, and `{to}` is not a member"
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "workspace architecture violated:\n  - {}",
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
            "`{from}` is not a workspace member"
        );
        assert!(graph.contains_key(*to), "`{to}` is not a workspace member");
        if reachable(&graph, from).contains(to) {
            let path = shortest_path(&graph, from, to).unwrap_or_else(|| "<unknown>".to_owned());
            violations.push(format!("`{from}` -> `{to}` ({rule}). path: {path}"));
        }
    }

    assert!(
        violations.is_empty(),
        "layering inverted:\n  - {}",
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
        "a cycle in the production dependencies:\n  - {}",
        violations.join("\n  - ")
    );
}
