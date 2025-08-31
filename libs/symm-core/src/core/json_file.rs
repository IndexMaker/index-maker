use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufReader, Read, Write},
};

/// Read bytes from file - synchronous version
pub fn read_file(path: &str) -> eyre::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;
    Ok(buffer)
}

/// Read from json file - synchronous version
pub fn read_from_json_file<T: for<'a> Deserialize<'a>>(path: &str) -> eyre::Result<T> {
    let data = read_file(path)?;
    let value: T = serde_json::from_slice(&data)?;
    Ok(value)
}

/// Write to json file - synchronous version
pub async fn write_json_to_file<T: Serialize>(path: &str, data: &T) -> eyre::Result<()> {
    let data = serde_json::to_string_pretty(data)?;
    let mut file = File::create(path)?;
    file.write_all(data.as_bytes())?;
    Ok(())
}