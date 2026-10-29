//! Facts embedded in this executable, independent of adjacent assets or stores.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn contract(bytes: &[u8], schema_id: bool) -> Value {
    let mut descriptor = json!({"sha256":format!("{:x}", Sha256::digest(bytes))});
    if schema_id {
        let schema: Value = serde_json::from_slice(bytes).expect("compiled protocol schema");
        descriptor["id"] = schema["$id"].clone();
    }
    descriptor
}

pub fn build_info() -> Value {
    let native = include_bytes!("../collectors/native/manifest.json");
    let manifest: Value = serde_json::from_slice(native).expect("compiled native manifest");
    let mut native = contract(native, false);
    native["version"] = manifest["version"].clone();
    native["extension"] = manifest["extension"].clone();
    json!({
        "schema_version":1,
        "app":{
            "name":env!("CARGO_PKG_NAME"),
            "version":env!("CARGO_PKG_VERSION"),
            "target":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH},
            // The checkout used by a later packager cannot establish which
            // source revision produced these executable bytes.
            "source_revision":null,
        },
        "contracts":{
            "record":contract(include_bytes!("../protocol/record.schema.json"),true),
            "responses":contract(include_bytes!("../protocol/response.schema.json"),true),
            "operations":contract(include_bytes!("../protocol/operations.json"),false),
            "native_manifest":native,
        },
        "storage":{"maximum_schema_version":crate::storage::SCHEMA_VERSION},
    })
}
