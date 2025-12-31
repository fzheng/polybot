//! Data recorder for backtesting

#![allow(dead_code)]

use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use crate::config::RecordingConfig;
use crate::types::PriceSnapshot;

/// Price data recorder
pub struct Recorder {
    config: RecordingConfig,
    current_file: Option<BufWriter<File>>,
    current_date: Option<String>,
    snapshots_written: u64,
}

impl Recorder {
    /// Create a new recorder
    pub fn new(config: RecordingConfig) -> Result<Self> {
        // Create data directory if it doesn't exist
        if config.enabled {
            fs::create_dir_all(&config.data_dir)
                .context("Failed to create data directory")?;
        }

        Ok(Self {
            config,
            current_file: None,
            current_date: None,
            snapshots_written: 0,
        })
    }

    /// Record a price snapshot
    pub fn record(&mut self, snapshot: &PriceSnapshot) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Check if we need to rotate file (new day)
        let date = snapshot.timestamp.format("%Y-%m-%d").to_string();
        if self.current_date.as_ref() != Some(&date) {
            self.rotate_file(&date)?;
        }

        // Write snapshot
        if let Some(writer) = &mut self.current_file {
            let json = serde_json::to_string(snapshot)?;
            writeln!(writer, "{}", json)?;
            self.snapshots_written += 1;

            // Flush every 100 snapshots
            if self.snapshots_written % 100 == 0 {
                writer.flush()?;
            }
        }

        Ok(())
    }

    /// Rotate to a new file
    fn rotate_file(&mut self, date: &str) -> Result<()> {
        // Flush and close current file
        if let Some(writer) = &mut self.current_file {
            writer.flush()?;
        }

        // Create new file
        let filename = format!("prices_{}.jsonl", date);
        let path = PathBuf::from(&self.config.data_dir).join(filename);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .context("Failed to open recording file")?;

        self.current_file = Some(BufWriter::new(file));
        self.current_date = Some(date.to_string());

        tracing::info!("Recording to: {}", path.display());

        Ok(())
    }

    /// Flush any buffered data
    pub fn flush(&mut self) -> Result<()> {
        if let Some(writer) = &mut self.current_file {
            writer.flush()?;
        }
        Ok(())
    }

    /// Get number of snapshots written
    pub fn snapshots_written(&self) -> u64 {
        self.snapshots_written
    }

    /// Check if recording is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

/// Load recorded snapshots from a file
pub fn load_snapshots(path: &str) -> Result<Vec<PriceSnapshot>> {
    let content = fs::read_to_string(path)?;
    let mut snapshots = Vec::new();

    for line in content.lines() {
        if !line.trim().is_empty() {
            let snapshot: PriceSnapshot = serde_json::from_str(line)?;
            snapshots.push(snapshot);
        }
    }

    Ok(snapshots)
}

/// List all recording files in the data directory
pub fn list_recordings(data_dir: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    for entry in fs::read_dir(data_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            files.push(path);
        }
    }

    files.sort();
    Ok(files)
}
