/// Strip markdown code fences from a string before JSON parsing.
/// Handles ```json, ``` prefix/suffix, and surrounding whitespace.
pub fn clean_json(s: &str) -> &str {
    let s = s.trim();
    let s = s.strip_prefix("```json").unwrap_or(s);
    let s = s.strip_prefix("```").unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    s.trim()
}

/// Replace literal newlines inside JSON string values with spaces.
/// This fixes models that emit newlines inside JSON string fields.
pub fn sanitize_json_strings(s: &str) -> String {
    s.replace("\r\n", "\\n").replace('\n', "\\n")
}

/// Parse a JSON string into T, stripping code fences first.
/// Returns a descriptive error including the cleaned input on failure.
pub fn parse_json<T: serde::de::DeserializeOwned>(s: &str) -> anyhow::Result<T> {
    let cleaned = clean_json(s);
    let sanitized = sanitize_json_strings(cleaned);
    serde_json::from_str(&sanitized).map_err(|e| {
        anyhow::anyhow!(
            "Failed to parse JSON: {}\nCleaned input was:\n{}",
            e,
            &sanitized[..sanitized.len().min(500)]
        )
    })
}
