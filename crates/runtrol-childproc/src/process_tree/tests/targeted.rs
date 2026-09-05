use std::collections::BTreeMap;

#[cfg(windows)]
use super::ProcessTree;
use super::{
    MAX_ANCESTOR_DEPTH, ProcessIdentity, selected_capture, selected_nodes, within, within_identity,
};

#[test]
fn snapshot_is_bracketed_by_the_same_seed_identity() {
    let taken = std::cell::Cell::new(false);
    let queried = std::cell::RefCell::new(Vec::new());
    let nodes = selected_capture(
        [30],
        || {
            assert_eq!(*queried.borrow(), [(30, false)]);
            taken.set(true);
            Ok(BTreeMap::from([(10, 0), (30, 10)]))
        },
        |pid| {
            queried.borrow_mut().push((pid, taken.get()));
            Some(if taken.get() { 301 } else { 300 })
        },
    )
    .expect("the snapshot itself succeeded");
    assert!(nodes.is_empty());
    assert_eq!(*queried.borrow(), [(30, false), (30, true)]);
}

#[test]
fn selected_capture_opens_only_seeds_and_their_ancestors() {
    let anchors = BTreeMap::from([(30, 300)]);
    let parents = BTreeMap::from([(10, 0), (20, 10), (30, 20), (99, 0)]);
    let starts = BTreeMap::from([(10, 100), (20, 200), (30, 300), (99, 90)]);
    let mut queried = Vec::new();
    let nodes = selected_nodes(&anchors, &parents, |pid| {
        queried.push(pid);
        starts.get(&pid).copied()
    });
    assert_eq!(queried, [30, 20, 10]);
    assert_eq!(nodes.keys().copied().collect::<Vec<_>>(), [10, 20, 30]);
    let root = ProcessIdentity::new(10, 100).expect("the root has an identity");
    let seed = ProcessIdentity::new(30, 300).expect("the seed has an identity");
    assert!(within_identity(root, seed, |pid| nodes.get(&pid).copied()));
    assert!(!within(99, 30, |pid| nodes.get(&pid).copied()));
}

#[test]
fn a_seed_reused_or_gone_after_anchoring_cannot_inherit_snapshot_edges() {
    let anchors = BTreeMap::from([(30, 300)]);
    let parents = BTreeMap::from([(10, 0), (30, 10)]);
    for after in [None, Some(301)] {
        let mut queried = Vec::new();
        let nodes = selected_nodes(&anchors, &parents, |pid| {
            queried.push(pid);
            after
        });
        assert!(nodes.is_empty());
        assert_eq!(
            queried,
            [30],
            "an unanchored seed never opens its old parent"
        );
    }
}

#[test]
fn a_recycled_ancestor_cannot_extend_a_selected_chain() {
    let anchors = BTreeMap::from([(30, 300)]);
    let parents = BTreeMap::from([(10, 0), (20, 10), (30, 20)]);
    let starts = BTreeMap::from([(10, 100), (20, 400), (30, 300)]);
    let mut queried = Vec::new();
    let nodes = selected_nodes(&anchors, &parents, |pid| {
        queried.push(pid);
        starts.get(&pid).copied()
    });
    assert_eq!(queried, [30, 20]);
    assert_eq!(nodes.keys().copied().collect::<Vec<_>>(), [30]);
    assert!(!within(10, 30, |pid| nodes.get(&pid).copied()));
}

#[test]
fn several_seeds_share_one_inspection_of_each_ancestor() {
    let anchors = BTreeMap::from([(30, 300), (40, 400)]);
    let parents = BTreeMap::from([(10, 0), (20, 10), (30, 20), (40, 20)]);
    let starts = BTreeMap::from([(10, 100), (20, 200), (30, 300), (40, 400)]);
    let mut queried = Vec::new();
    let nodes = selected_nodes(&anchors, &parents, |pid| {
        queried.push(pid);
        starts.get(&pid).copied()
    });
    assert_eq!(queried, [30, 20, 10, 40]);
    assert!(within(10, 30, |pid| nodes.get(&pid).copied()));
    assert!(within(10, 40, |pid| nodes.get(&pid).copied()));
}

#[test]
fn selected_capture_stops_at_cycles_and_the_existing_depth_bound() {
    let anchors = BTreeMap::from([(1, 100)]);
    let cycle = BTreeMap::from([(1, 2), (2, 3), (3, 1)]);
    let nodes = selected_nodes(&anchors, &cycle, |_| Some(100));
    assert_eq!(nodes.len(), 3);
    assert!(!within(99, 1, |pid| nodes.get(&pid).copied()));

    let last = u32::try_from(MAX_ANCESTOR_DEPTH).expect("the depth fits a PID") + 1;
    let parents = (1..=last + 5).map(|pid| (pid, pid + 1)).collect();
    let mut queried = Vec::new();
    let nodes = selected_nodes(&anchors, &parents, |pid| {
        queried.push(pid);
        Some(100)
    });
    assert_eq!(nodes.len(), MAX_ANCESTOR_DEPTH + 1);
    assert_eq!(queried.len(), MAX_ANCESTOR_DEPTH + 1);
    assert!(within(last, 1, |pid| nodes.get(&pid).copied()));
    assert!(!within(last + 1, 1, |pid| nodes.get(&pid).copied()));
}

#[cfg(windows)]
#[test]
fn selected_nodes_still_recheck_birth_when_used() {
    let current = super::process_identity(std::process::id()).expect("the current process exists");
    let mut tree =
        ProcessTree::capture_for([current.pid()]).expect("the selected capture succeeds");
    assert!(tree.contains_identity(current, current));
    tree.nodes
        .get_mut(&current.pid())
        .expect("the seed was retained")
        .started += 1;
    assert!(!tree.contains(current.pid(), current.pid()));
    assert!(!tree.contains_identity(current, current));
}

#[cfg(windows)]
#[test]
fn an_empty_selection_is_empty_and_unbounded_input_is_refused() {
    assert!(
        ProcessTree::capture_for([0])
            .expect("zero selects nothing")
            .nodes
            .is_empty()
    );
    assert!(ProcessTree::capture_for(std::iter::repeat(0)).is_err());
}

/// Manual, body-free cost comparison using the same public capture calls as the Runtime.
#[cfg(windows)]
#[test]
#[ignore = "manual operating-system cost probe; ordinary tests assert ancestry instead of machine timing"]
fn compare_full_and_selected_capture_cost() {
    let current = super::process_identity(std::process::id()).expect("the current process exists");
    for selected in [false, true] {
        let cpu = process_cpu();
        let started = std::time::Instant::now();
        let mut retained = 0;
        for _ in 0..5 {
            let tree = if selected {
                ProcessTree::capture_for([current.pid()])
            } else {
                ProcessTree::capture()
            }
            .expect("the process table can be inspected");
            assert!(tree.contains_identity(current, current));
            retained = tree.nodes.len();
        }
        println!(
            "selected={selected} captures=5 retained={retained} wall_ms={} cpu_ms={}",
            started.elapsed().as_millis(),
            process_cpu().saturating_sub(cpu).as_millis()
        );
    }
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "the manual probe reads only its own process CPU counters"
)]
fn process_cpu() -> std::time::Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the pseudo-handle names this process and all four output pointers are writable FILETIMEs.
    let read = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    };
    assert_ne!(read, 0, "the current process CPU counters are readable");
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    std::time::Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}
