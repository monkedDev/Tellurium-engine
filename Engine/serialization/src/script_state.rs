use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ScriptState;

// Bound input nesting with serde_json's recursion limit even inside binary documents.
pub fn serialize<S: Serializer>(
    value: &Option<ScriptState>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if serializer.is_human_readable() {
        value.serialize(serializer)
    } else {
        let data = value
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(serde::ser::Error::custom)?;
        data.serialize(serializer)
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ScriptState>, D::Error> {
    if deserializer.is_human_readable() {
        Option::<ScriptState>::deserialize(deserializer)
    } else {
        let data = Option::<String>::deserialize(deserializer)?;
        data.as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(serde::de::Error::custom)
    }
}
