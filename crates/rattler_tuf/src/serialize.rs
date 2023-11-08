use serde_json::{to_string_pretty, Value};

pub fn canonserialize(obj: &Value) -> Result<Vec<u8>, serde_json::Error> {
    // Serialize the object to a pretty JSON string with an indentation of 2 spaces
    let pretty_json = to_string_pretty(&obj)?;
    // Convert the JSON string to a utf-8 encoded vector of bytes
    Ok(pretty_json.into_bytes())
}
