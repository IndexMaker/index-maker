use serde::{Deserialize, Serialize};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
};

/// Read bytes from file - asynchronous version
pub async fn read_file_async(path: &str) -> eyre::Result<Vec<u8>> {
    let file = File::open(path).await?;
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;
    Ok(buffer)
}

/// Read from json file - asynchronous version
pub async fn read_from_json_file_async<T: for<'a> Deserialize<'a>>(path: &str) -> eyre::Result<T> {
    let data = read_file_async(path).await?;
    let value: T = serde_json::from_slice(&data)?;
    Ok(value)
}

/// Write to json file - asynchronous version
pub async fn write_json_to_file_async<T: Serialize>(path: &str, data: &T) -> eyre::Result<()> {
    let data = serde_json::to_string_pretty(data)?;
    let mut file = File::create(path).await?;
    file.write_all(data.as_bytes()).await?;
    Ok(())
}
