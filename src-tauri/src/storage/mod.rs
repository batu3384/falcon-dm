pub mod database;
pub mod models;
mod queries;

pub use database::{Database, DatabaseError, Result};
pub use models::{Download, DownloadCategory, DownloadFilter, DownloadStatus};
