use serde_json::{Map, Value};

/// Normalize model catalog aliases before the pinned ACP SDK deserializes a
/// JSON-RPC line. The SDK requires `modelId`; newer and third-party agents may
/// emit `id` or `model_id`, and malformed entries must not invalidate the rest
/// of an otherwise usable catalog.
pub(super) fn normalize_incoming_line(line: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(line) else {
        return line.to_owned();
    };
    if !normalize_value(&mut value) {
        return line.to_owned();
    }
    serde_json::to_string(&value).unwrap_or_else(|_| line.to_owned())
}

fn normalize_value(value: &mut Value) -> bool {
    match value {
        Value::Object(object) => normalize_object(object),
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| normalize_value(item) || changed),
        _ => false,
    }
}

fn normalize_object(object: &mut Map<String, Value>) -> bool {
    let mut changed = false;

    if !object.contains_key("availableModels")
        && let Some(models) = object.remove("available_models")
    {
        object.insert("availableModels".to_owned(), models);
        changed = true;
    }

    if let Some(Value::Array(models)) = object.get_mut("availableModels") {
        let previous_len = models.len();
        models.retain_mut(|model| {
            let Value::Object(model) = model else {
                return false;
            };
            if !model.contains_key("modelId") {
                if let Some(id) = model.remove("model_id").or_else(|| model.remove("id")) {
                    model.insert("modelId".to_owned(), id);
                    changed = true;
                }
            }
            model
                .get("modelId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.trim().is_empty())
        });
        changed |= models.len() != previous_len;
    }

    for value in object.values_mut() {
        changed |= normalize_value(value);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_aliases_are_normalized_without_dropping_valid_entries() {
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"availableModels":[{"id":"opus","name":"Opus"},{"model_id":"sonnet","name":"Sonnet"},{"name":"broken"}]}}}"#;
        let normalized: Value = serde_json::from_str(&normalize_incoming_line(line)).unwrap();
        let models = normalized
            .pointer("/params/update/availableModels")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["modelId"], "opus");
        assert_eq!(models[1]["modelId"], "sonnet");
    }

    #[test]
    fn unrelated_and_malformed_lines_are_forwarded_verbatim() {
        let unrelated = r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"text":"hello"}}}"#;
        assert_eq!(normalize_incoming_line(unrelated), unrelated);
        assert_eq!(normalize_incoming_line("not-json"), "not-json");
    }
}
