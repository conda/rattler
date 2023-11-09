use std::{collections::BTreeMap, path::Path};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
    pub fn canonical_serialize(&self) -> Result<Vec<u8>, serde_json::Error> {
        // Serialize the object to a pretty JSON string with an indentation of 2 spaces
        let pretty_json = serde_json::to_string_pretty(&self)?;
        // Convert the JSON string to a utf-8 encoded vector of bytes
        Ok(pretty_json.into_bytes())
    }

    pub fn hash(&self) -> String {
        let serialized = self.canonical_serialize().unwrap();

        // compute sha256 hash of the serialized payload
        let mut hasher = Sha256::new();
        hasher.update(serialized);
        let result = hasher.finalize();

        format!("{:x}", result)
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

        let serialized = payload.canonical_serialize().unwrap();

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
pub struct Root {
    pub signatures: BTreeMap<PublicKey, Signature>,
    pub signed: Payload,
}

impl Root {
    pub fn try_from_file(path: &Path) -> Result<Self, std::io::Error> {
        let root_str = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&root_str)?)
    }

    pub fn verify_signatures(&self) -> Result<(), std::io::Error> {
        for (pubkey, sig) in &self.signatures {
            if !pubkey.verify(&self.signed, sig) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Bad signature",
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use crate::model::Root;

    fn test_data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-data")
    }

    #[test]
    fn test_root() {
        let root_path = test_data_dir().join("demo/1.root.json");
        let root = Root::try_from_file(&root_path).unwrap();

        let payload_hash = root.signed.hash();
        assert_eq!(
            payload_hash,
            "8f264e8c9bb38ec36700a2502ef58a39357594ba754089215687344118391c48"
        );

        let (pubkey, sig) = root.signatures.first_key_value().unwrap();
        let verified = pubkey.verify(&root.signed, sig);
        assert!(verified);

        let root_path_2 = test_data_dir().join("demo/2.root.json");
        let mut root = Root::try_from_file(&root_path_2).unwrap();
        assert!(root.verify_signatures().is_ok());

        // make sure that the signatures are invalid if we change the payload
        root.signed.timestamp = chrono::Utc::now();
        assert!(root.verify_signatures().is_err());
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

        let serialized = root.signed.canonical_serialize().unwrap();
        let serialized_str = std::str::from_utf8(&serialized).unwrap();
        insta::assert_snapshot!(serialized_str);
    }
}
