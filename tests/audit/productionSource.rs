/// Return the production prefix of a Rust source file whose conventional test module is the final item.
///
/// Small test-only fields or methods may appear inside production items, so the first `cfg(test)` attribute is not a
/// safe boundary. Repository Rust modules keep their full `mod tests` block last, which gives source audits one exact
/// boundary without treating fixture-only calls as product authority.
pub(crate) fn without_tail_test_module(source: &str) -> &str {
    const LF_MARKER: &str = "#[cfg(test)]\nmod tests";
    const CRLF_MARKER: &str = "#[cfg(test)]\r\nmod tests";
    let end = [source.find(LF_MARKER), source.find(CRLF_MARKER)]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(source.len());
    &source[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_test_modules_are_removed_for_both_line_endings() {
        for source in [
            "fn product() {}\n#[cfg(test)]\nmod tests { fn fixture() {} }",
            "fn product() {}\r\n#[cfg(test)]\r\nmod tests { fn fixture() {} }",
        ] {
            assert_eq!(
                without_tail_test_module(source).trim_end(),
                "fn product() {}"
            );
        }
    }

    #[test]
    fn a_small_test_only_item_does_not_hide_later_product_code() {
        let source = "#[cfg(test)]\nfn helper() {}\nfn product() {}";
        assert_eq!(without_tail_test_module(source), source);
    }
}
