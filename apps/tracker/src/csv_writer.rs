use std::{collections::HashMap, fs::OpenOptions};

use itertools::Itertools;

pub struct CsvWriter {
    headers: Vec<String>,
    rows: Vec<HashMap<String, String>>,
}

impl CsvWriter {
    pub fn new(headers: Vec<impl Into<String>>) -> CsvWriter {
        CsvWriter {
            headers: headers.into_iter().map_into().collect_vec(),
            rows: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn push(&mut self, row: HashMap<String, String>) {
        self.rows.push(row);
    }

    pub fn write_into_file(self, path: &str) -> eyre::Result<()> {
        // open in append mode, create if missing
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;

        // if file length is zero, we need to write headers once
        let write_header = file.metadata()?.len() == 0;

        // IMPORTANT: has_headers(false) because we’re writing records manually
        let mut writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(&mut file);

        if write_header {
            writer.write_record(&self.headers)?;
        }

        for row in self.rows {
            let record: Vec<String> = self
                .headers
                .iter()
                .map(|h| row.get(h).cloned().unwrap_or_default())
                .collect();

            writer.write_record(&record)?;
        }

        writer.flush()?;
        // file is dropped here and flushed to disk
        Ok(())
    }
}
