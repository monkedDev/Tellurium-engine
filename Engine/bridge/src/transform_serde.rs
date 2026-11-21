use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Transform;

#[derive(Serialize, Deserialize)]
#[serde(remote = "Transform")]
struct Data {
    position: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}

impl Serialize for Transform {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Data::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for Transform {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Data::deserialize(deserializer)
    }
}
