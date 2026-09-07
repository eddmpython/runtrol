#![cfg(test)]

use super::*;

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("../fixtures/queries.json")).expect("valid agreement fixture")
}

fn strings(fixture: &serde_json::Value, key: &str) -> Vec<String> {
    serde_json::from_value(fixture.get(key).expect("fixture field").clone()).expect("string array")
}

#[test]
fn agreement_fixture_is_the_rust_grammar_not_a_second_query_list() {
    let fixture = fixture();
    assert_eq!(
        fixture.get("carryLimit"),
        Some(&serde_json::json!(CARRY_LIMIT))
    );
    assert_eq!(
        fixture.get("modeDigits"),
        Some(&serde_json::json!(MODE_DIGITS))
    );
    let literals: Vec<_> = LITERALS
        .iter()
        .map(|(literal, _)| std::str::from_utf8(literal).expect("ASCII VT"))
        .collect();
    assert_eq!(strings(&fixture, "literalBodies"), literals);
    assert_eq!(
        fixture
            .get("replacement")
            .and_then(serde_json::Value::as_str)
            .expect("replacement")
            .as_bytes(),
        QUERY_TERMINATOR
    );
    let mut queries: Vec<_> = literals.iter().map(|body| format!("\x1b{body}")).collect();
    for mode in [0, 1, 2026, 99_999] {
        queries.push(format!("\x1b[?{mode}$p"));
    }
    for colour in [10, 11] {
        for terminator in ["\x07", "\x1b\\"] {
            queries.push(format!("\x1b]{colour};?{terminator}"));
        }
    }
    for body in ["", "524742", "544e;436f", "abcd"] {
        queries.push(format!("\x1bP+q{body}\x1b\\"));
    }
    assert_eq!(strings(&fixture, "queries"), queries);
}

#[test]
fn every_owned_query_is_consumed_whole_or_split_without_changing_drawing() {
    for query in strings(&fixture(), "queries") {
        for split in 0..=query.len() {
            let mut filter = QueryFilter::default();
            let (first, second) = query.as_bytes().split_at(split);
            let mut drawn = filter.filter(&[b"before".as_slice(), first].concat());
            drawn.extend(filter.filter(&[second, b"after"].concat()));
            drawn.extend(filter.finish());
            assert_eq!(drawn, b"before\x1b\\after", "split {split}, {query:?}");
        }
        let mut filter = QueryFilter::default();
        let mut drawn: Vec<_> = query
            .bytes()
            .flat_map(|byte| filter.filter(&[byte]))
            .collect();
        drawn.extend(filter.finish());
        assert_eq!(drawn, QUERY_TERMINATOR, "one byte per frame, {query:?}");
    }
}

#[test]
fn unknown_vt_and_oversized_candidates_remain_exact_through_every_split() {
    for bytes in strings(&fixture(), "passThrough") {
        for split in 0..=bytes.len() {
            let (first, second) = bytes.as_bytes().split_at(split);
            let mut filter = QueryFilter::default();
            let mut drawn = filter.filter(first);
            drawn.extend(filter.filter(second));
            drawn.extend(filter.finish());
            assert_eq!(drawn, bytes.as_bytes(), "split {split}");
        }
    }
}

#[test]
fn finishing_a_stream_never_carries_its_fragment_into_the_next_view() {
    let mut filter = QueryFilter::default();
    assert!(filter.filter(b"\x1b[?2").is_empty());
    assert_eq!(filter.finish(), b"\x1b[?2");
    assert_eq!(filter.filter(b"026$p"), b"026$p");
    assert_eq!(
        filter.filter(b"\x1bP+q123\x1b[6ntext"),
        b"\x1bP+q123\x1b\\text"
    );
}

#[test]
fn query_replacement_preserves_csi_cancellation_and_osc_dcs_completion() {
    for item in fixture()
        .get("cancellation")
        .expect("cancellation corpus")
        .as_array()
        .expect("array")
    {
        let before = item
            .get("before")
            .and_then(serde_json::Value::as_str)
            .expect("prefix");
        let after = item
            .get("after")
            .and_then(serde_json::Value::as_str)
            .expect("suffix");
        let raw = format!("{before}\x1b[6n{after}");
        for split in 0..=raw.len() {
            let mut filter = QueryFilter::default();
            let (first, second) = raw.as_bytes().split_at(split);
            let mut drawn = filter.filter(first);
            drawn.extend(filter.filter(second));
            drawn.extend(filter.finish());
            assert_eq!(drawn, format!("{before}\x1b\\{after}").as_bytes());
        }
    }
}
