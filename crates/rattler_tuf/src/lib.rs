use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
mod serialize;

use ed25519_dalek::{Signature as Ed25519Signature, Verifier, VerifyingKey};
use hex::FromHex;
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize, Deserialize)]
pub struct Signature {
    other_headers: Option<String>,
    signature: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Version(u32);

#[derive(Debug, Serialize, Deserialize)]
pub struct MetadataSpecVersion(String);

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Type {
    Root,
    KeyMgr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Payload {
    pub delegations: BTreeMap<String, Delegation>,
    pub expiration: DateTime<Utc>,
    pub metadata_spec_version: MetadataSpecVersion,
    pub timestamp: DateTime<Utc>,
    #[serde(rename = "type")]
    pub file_type: Type,
    pub version: Version,
}

impl Payload {
    pub fn hash(&self) -> String {
        let serialized = serialize::canonserialize(&serde_json::to_value(self).unwrap()).unwrap();
        // compute sha256 hash of the serialized payload
        let mut hasher = Sha256::new();

        // Write input message
        hasher.update(serialized);

        // Read hash digest and consume hasher
        let result = hasher.finalize();
        // convert the hash to a hex string
        let hex_string = format!("{:x}", result);

        hex_string
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, PartialOrd, Ord, Eq)]
pub struct PublicKey(String);

impl PublicKey {
    pub fn verify(&self, payload: &Payload, signature: &Signature) -> bool {
        let public_key_bytes = <[u8; 32]>::from_hex(&self.0).unwrap();

        let verifying_key = VerifyingKey::from_bytes(&public_key_bytes).unwrap();
        let signature_bytes = hex::decode(&signature.signature).unwrap();
        let ed_signature = Ed25519Signature::try_from(signature_bytes.as_slice()).unwrap();

        let serialized =
            serialize::canonserialize(&serde_json::to_value(payload).unwrap()).unwrap();

        if let Some(other_headers) = &signature.other_headers {
            // in this case we need to hash the payload with the additional header data
            // to make the signature GPG compatible
            let additional_header_data = hex::decode(other_headers).unwrap();

            let mut hasher = Sha256::new();
            hasher.update(&serialized);
            hasher.update(&additional_header_data);
            hasher.update(b"\x04\xff");
            hasher.update((additional_header_data.len() as u32).to_be_bytes());

            let combined_hash = hasher.finalize();

            verifying_key.verify(&combined_hash, &ed_signature).is_ok()
        } else {
            verifying_key.verify(&serialized, &ed_signature).is_ok()
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Delegation {
    pub pubkeys: Vec<PublicKey>,
    pub threshold: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct Root {
    pub signatures: BTreeMap<PublicKey, Signature>,
    pub signed: Payload,
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    fn test_data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-data")
    }

    #[test]
    fn test_root() {
        let root_path = test_data_dir().join("demo/1.root.json");
        let root_str = std::fs::read_to_string(root_path).unwrap();
        let root: super::Root = serde_json::from_str(&root_str).unwrap();

        let payload_hash = root.signed.hash();
        assert_eq!(
            payload_hash,
            "8f264e8c9bb38ec36700a2502ef58a39357594ba754089215687344118391c48"
        );

        let (pubkey, sig) = root.signatures.first_key_value().unwrap();
        let verified = pubkey.verify(&root.signed, sig);
        assert!(verified);
    }

    #[test]
    fn test_key_mgr() {
        let keymgr_path = test_data_dir().join("demo/key_mgr.json");
        let keymgr_str = std::fs::read_to_string(keymgr_path).unwrap();
        let keymgr: super::Root = serde_json::from_str(&keymgr_str).unwrap();

        let payload_hash = keymgr.signed.hash();
        assert_eq!(
            payload_hash,
            "fe15c8fcb4e6147ab32a4957db6cceb4a089162d2cb0400e0fbb8f1babeae6ef"
        );

        let (pubkey, sig) = keymgr.signatures.first_key_value().unwrap();
        let verified = pubkey.verify(&keymgr.signed, sig);
        assert!(verified);
    }

    #[test]
    fn test_canonicalize() {
        let root_path = test_data_dir().join("demo/1.root.json");
        let root_str = std::fs::read_to_string(root_path).unwrap();
        let root: super::Root = serde_json::from_str(&root_str).unwrap();

        let serialized =
            super::serialize::canonserialize(&serde_json::to_value(&root.signed).unwrap()).unwrap();
        let serialized_str = std::str::from_utf8(&serialized).unwrap();
        insta::assert_snapshot!(serialized_str);
    }
}
