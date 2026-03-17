use rocksdb::{ColumnFamilyDescriptor, DBWithThreadMode, Options, SingleThreaded, WriteBatch, DB};
use std::env;

#[derive(Debug)]
pub struct Database {
    db: Option<DBWithThreadMode<SingleThreaded>>,
}

impl Database {
    fn db_path(name: &str) -> String {
        let db_base_path = env::var("DB_PATH").unwrap_or_else(|_| {
            let current_dir = env::current_dir().expect("Failed to get current directory");
            current_dir.to_str().unwrap_or(".").to_string()
        });
        format!("{}/{}.db", db_base_path, name)
    }

    pub fn new_db(name: &str) -> Self {
        let db_path = Database::db_path(&name);
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        let cf_descriptors = vec![
            ColumnFamilyDescriptor::new("block", Options::default()),
            ColumnFamilyDescriptor::new("state", Options::default()),
            ColumnFamilyDescriptor::new("blockchain", Options::default()),
            ColumnFamilyDescriptor::new("tx_pool", Options::default()),            
        ];

        let db = DBWithThreadMode::<SingleThreaded>::open_cf_descriptors(
            &options,
            &db_path,
            cf_descriptors,
        )
        .expect("Failed to open database with specified column families");

        Database { db: Some(db) }
    }

    pub fn get(&self, cf_name: &str, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        match &self.db {
            Some(db) => {
                let cf_handle = db.cf_handle(cf_name).ok_or("Column family not found")?;
                db.get_cf(cf_handle, key).map_err(|e| e.to_string())
            }
            None => Err("Database connection is closed".to_string()),
        }
    }

    pub fn put(&self, cf_name: &str, key: &[u8], value: &[u8]) -> Result<(), String> {
        match &self.db {
            Some(db) => {
                let cf_handle = db.cf_handle(cf_name).ok_or("Column family not found")?;
                db.put_cf(cf_handle, key, value).map_err(|e| e.to_string())
            }
            None => Err("Database connection is closed".to_string()),
        }
    }

    pub fn delete(&self, cf_name: &str, key: &[u8]) -> Result<(), String> {
        match &self.db {
            Some(db) => {
                let cf_handle = db.cf_handle(cf_name).ok_or("Column family not found")?;
                db.delete_cf(cf_handle, key).map_err(|e| e.to_string())
            }
            None => Err("Database connection is closed".to_string()),
        }
    }

    pub fn write(&self, operations: Vec<(&str, &[u8], Option<&[u8]>)>) -> Result<(), String> {
        let mut batch = WriteBatch::default();
    
        match &self.db {
            Some(db) => {
                for (cf_name, key, value) in operations {
                    let cf_handle = db
                        .cf_handle(cf_name)
                        .ok_or(format!("Column family {} not found", cf_name))?;
                    
                    match value {
                        Some(v) => batch.put_cf(cf_handle, key, v),
                        None => batch.delete_cf(cf_handle, key),
                    };
                }
                db.write(batch).map_err(|e| e.to_string())
            }
            None => Err("Database connection is closed".to_string()),
        }
    }
    

    pub fn close(&mut self) {
        let _ = self.db.take(); // Properly drops the database object, closing the connection
    }

    pub fn delete_database(&self, name: &str) -> Result<(), String> {
        let db_path = Database::db_path(&name);

        DB::destroy(&Options::default(), db_path).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_keys_by_cf_name(&self, cf_name: &str) -> Result<Vec<Vec<u8>>, String> {
        match &self.db {
            Some(db) => {
                let cf_handle = db
                    .cf_handle(cf_name)
                    .ok_or(format!("Column family '{}' not found", cf_name))?;
                let mut keys = Vec::new();
                let iter = db.prefix_iterator_cf(cf_handle, ""); // Using prefix_iterator_cf with empty prefix to get all keys

                for item in iter {
                    match item {
                        Ok((key, _)) => keys.push(key.to_vec()), // Collect keys, ignore values
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Ok(keys)
            }
            None => Err("Database connection is closed".to_string()),
        }
    }

    pub fn get_keys_values_by_cf_name(
        &self,
        cf_name: &str,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        match &self.db {
            Some(db) => {
                let cf_handle = db
                    .cf_handle(cf_name)
                    .ok_or(format!("Column family '{}' not found", cf_name))?;
                let mut entries = Vec::new();
                let iter = db.prefix_iterator_cf(cf_handle, ""); // Using prefix_iterator_cf with empty prefix to get all entries

                for item in iter {
                    match item {
                        Ok((key, value)) => entries.push((key.to_vec(), value.to_vec())), // Collect keys and values
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Ok(entries)
            }
            None => Err("Database connection is closed".to_string()),
        }
    }

    /// Iterate over key-value pairs in a column family whose keys start with the given prefix.
    pub fn prefix_scan(
        &self,
        cf_name: &str,
        prefix: &[u8],
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        match &self.db {
            Some(db) => {
                let cf_handle = db
                    .cf_handle(cf_name)
                    .ok_or(format!("Column family '{}' not found", cf_name))?;
                let mut entries = Vec::new();
                let iter = db.prefix_iterator_cf(cf_handle, prefix);

                for item in iter {
                    match item {
                        Ok((key, value)) => entries.push((key.to_vec(), value.to_vec())),
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Ok(entries)
            }
            None => Err("Database connection is closed".to_string()),
        }
    }
}
