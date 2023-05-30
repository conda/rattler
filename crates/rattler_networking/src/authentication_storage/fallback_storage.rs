use std::{path::PathBuf, sync::Mutex};

pub struct FallbackStorage {
    pub path: PathBuf,

    mutex: Mutex<()>,
}

#[derive(thiserror::Error, Debug)]
pub enum FallbackStorageError {
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JSONError(#[from] serde_json::Error),
}

impl FallbackStorage {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            mutex: Mutex::new(()),
        }
    }

    pub fn set_password(&self, host: &str, password: &str) -> Result<(), FallbackStorageError> {
        let _lock = self.mutex.lock().unwrap();
        let mut dict = self.read_json()?;
        dict.insert(host.to_string(), password.to_string());
        self.write_json(&dict)
    }

    pub fn get_password(&self, host: &str) -> Result<Option<String>, FallbackStorageError> {
        let _lock = self.mutex.lock().unwrap();
        let dict = self.read_json()?;
        println!("{} {:?}", host, dict);
        Ok(dict.get(host).cloned())
    }

    pub fn delete_password(&self, host: &str) -> Result<(), FallbackStorageError> {
        let _lock = self.mutex.lock().unwrap();
        let mut dict = self.read_json()?;
        dict.remove(host);
        self.write_json(&dict)
    }

    fn read_json(&self) -> Result<std::collections::HashMap<String, String>, FallbackStorageError> {
        if !self.path.exists() {
            return Ok(std::collections::HashMap::new());
        }
        let file = std::fs::File::open(&self.path)?;
        let reader = std::io::BufReader::new(file);
        let dict = serde_json::from_reader(reader)?;
        Ok(dict)
    }

    fn write_json(
        &self,
        dict: &std::collections::HashMap<String, String>,
    ) -> Result<(), FallbackStorageError> {
        if !self.path.exists() {
            std::fs::create_dir_all(self.path.parent().unwrap())?;
        }
        let file = std::fs::File::create(&self.path)?;
        let writer = std::io::BufWriter::new(file);
        serde_json::to_writer(writer, dict)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_fallback_storage() {
        let file = tempdir().unwrap();
        let path = file.path().join("test.json");

        let storage = FallbackStorage::new(path.clone());

        assert_eq!(storage.get_password("test").unwrap(), None);

        storage.set_password("test", "password").unwrap();
        assert_eq!(
            storage.get_password("test").unwrap(),
            Some("password".to_string())
        );

        storage.delete_password("test").unwrap();
        assert_eq!(storage.get_password("test").unwrap(), None);

        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"invalid json").unwrap();
        assert!(storage.get_password("test").is_err());
    }
}
