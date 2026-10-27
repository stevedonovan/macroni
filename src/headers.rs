/// Match a Content-Type media type, ignoring parameters and ASCII case.
/// Missing or non-text header values do not match.
pub(crate) fn has_content_type(headers: &http::HeaderMap, expected: &str) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case(expected)
        })
}
