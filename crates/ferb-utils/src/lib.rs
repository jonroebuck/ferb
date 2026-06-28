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
/// Falls back to extracting the first complete `{...}` block from mixed prose+JSON responses.
pub fn parse_json<T: serde::de::DeserializeOwned>(s: &str) -> anyhow::Result<T> {
    let cleaned = clean_json(s);
    let sanitized = sanitize_json_strings(cleaned);

    if let Ok(val) = serde_json::from_str::<T>(&sanitized) {
        return Ok(val);
    }

    // Extract first JSON object from prose+JSON mixed responses
    if let (Some(start), Some(end)) = (cleaned.find('{'), cleaned.rfind('}')) {
        if end > start {
            let candidate = sanitize_json_strings(&cleaned[start..=end]);
            if let Ok(val) = serde_json::from_str::<T>(&candidate) {
                eprintln!("[warn] parse_json: extracted JSON from mixed prose response");
                return Ok(val);
            }
        }
    }

    Err(anyhow::anyhow!(
        "Failed to parse JSON: all parse attempts failed\nCleaned input was:\n{}",
        &sanitized[..sanitized.len().min(500)]
    ))
}
