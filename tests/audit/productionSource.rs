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
