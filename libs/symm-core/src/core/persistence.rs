use eyre::Result;
use serde_json::Value;

pub trait Persist {
    fn load(&mut self) -> Result<()>;
    fn store(&self) -> Result<()>;
}

pub trait Persistence {
    fn load_value(&self) -> Result<Option<Value>>;
    fn store_value(&self, value: Value) -> Result<()>;
}

pub mod util {
    use std::{fs, path::PathBuf};

    use eyre::{Context, Result};
    use parking_lot::RwLock;
    use serde_json::Value;

    use crate::core::
        persistence::Persistence
    ;

    pub struct InMemoryPersistence {
        data: RwLock<Option<String>>,
    }

    impl InMemoryPersistence {
        pub fn new() -> Self {
            Self {
                data: RwLock::new(None),
            }
        }
    }

    impl Persistence for InMemoryPersistence {
        fn load_value(&self) -> Result<Option<Value>> {
            if let Some(json_string) = self.data.read().as_ref() {
                Ok(Some(
                    serde_json::from_str(json_string).context("Failed to deserialize")?,
                ))
            } else {
                Ok(None)
            }
        }

        fn store_value(&self, value: Value) -> Result<()> {
            let json_string = serde_json::to_string_pretty(&value)?;
            *self.data.write() = Some(json_string);
            Ok(())
        }
    }

    pub struct JsonFilePersistence {
        path: PathBuf,
    }

    impl JsonFilePersistence {
        pub fn new(path: impl Into<PathBuf>) -> Self {
            JsonFilePersistence { path: path.into() }
        }
    }

    impl Persistence for JsonFilePersistence {
        fn load_value(&self) -> Result<Option<Value>> {
            if !self.path.exists() {
                return Ok(None);
            }

            let json_string = fs::read_to_string(&self.path)?;
            Ok(Some(
                serde_json::from_str(&json_string).context("Failed to deserialize")?,
            ))
        }

        fn store_value(&self, value: Value) -> Result<()> {
            // Create the directory if it doesn't exist.
            if let Some(parent) = self.path.parent() {
                fs::create_dir_all(parent).context("Failed to create parent directory")?;
            }

            let json_string = serde_json::to_string_pretty(&value).context("Failed to serialize")?;
            fs::write(&self.path, &json_string).context("Failed to write json file")
        }
    }
}
