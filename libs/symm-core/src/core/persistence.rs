use eyre::Result;
use serde_json::Value;

pub trait Persist {
    fn load(&mut self) -> Result<()>;
    fn store(&self) -> Result<()>;
}

pub trait Persistence {
    fn load_value(&self) -> Result<Option<Value>>;
    fn store_value(&self, value: Value) -> Result<()>;
    fn child(&self, key: String) -> Result<Box<dyn Persistence>>;   // To allow nested persistence
}

pub mod util {
    use std::{collections::HashMap, fs, path::PathBuf, sync::Arc};

    use eyre::{Context, Result};
    use parking_lot::RwLock;
    use serde_json::Value;

    use crate::core::
        persistence::Persistence
    ;

    pub struct InMemoryPersistence {
        data: Arc<RwLock<HashMap<String, String>>>,  // Shared storage with keys
        prefix: String,  // Namespace for this instance
    }

    impl InMemoryPersistence {
        pub fn new() -> Self {
            Self {
                data: Arc::new(RwLock::new(HashMap::new())),
                prefix: String::new(),
            }
        }

        fn new_with_data(data: Arc<RwLock<HashMap<String, String>>>, prefix: String) -> Self {
            Self { data, prefix }
        }

        fn get_key(&self) -> String {
            if self.prefix.is_empty() {
                "root".to_string()
            } else {
                self.prefix.clone()
            }
        }
    }

    impl Persistence for InMemoryPersistence {
        fn load_value(&self) -> Result<Option<Value>> {
            let key = self.get_key();
            if let Some(json_string) = self.data.read().get(&key) {
                Ok(Some(
                    serde_json::from_str(json_string).context("Failed to deserialize")?,
                ))
            } else {
                Ok(None)
            }
        }

        fn store_value(&self, value: Value) -> Result<()> {
            let key = self.get_key();
            let json_string = serde_json::to_string_pretty(&value)?;
            self.data.write().insert(key, json_string);
            Ok(())
        }
        
        fn child(&self, key: String) -> Result<Box<dyn Persistence>> {
            let child_prefix = if self.prefix.is_empty() {
                key
            } else {
                format!("{}/{}", self.prefix, key)
            };
            Ok(Box::new(InMemoryPersistence::new_with_data(
                Arc::clone(&self.data),
                child_prefix,
            )))
        }
    }

    pub struct JsonFilePersistence {
        path: PathBuf,
    }

    impl JsonFilePersistence {
        pub fn new(path: impl Into<PathBuf>) -> Self {
            JsonFilePersistence { path: path.into() }
        }

        fn create_child_path(&self, key: &str) -> PathBuf {
            let mut parent_path = self.path.clone();
            
            // If parent has .json extension, remove it to use as directory
            if parent_path.extension().is_some() {
                parent_path.set_extension("");
            }

            // Add key as filename with .json extension
            parent_path.push(format!("{}.json", key));
            parent_path
        }
    }

    impl Persistence for JsonFilePersistence {
        fn load_value(&self) -> Result<Option<Value>> {
            if !self.path.exists() {
                return Ok(None);
            }

            tracing::info!("Loading {:#?}", self.path);
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

            tracing::info!("Storing {:#?}", self.path);
            let json_string = serde_json::to_string_pretty(&value).context("Failed to serialize")?;
            fs::write(&self.path, &json_string).context("Failed to write json file")
        }
        
        fn child(&self, key: String) -> Result<Box<dyn Persistence>> {
            let child_path = self.create_child_path(&key);
            Ok(Box::new(JsonFilePersistence::new(child_path)))
        }
    }
}
