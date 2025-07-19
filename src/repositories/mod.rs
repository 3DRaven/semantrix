use lancedb::{
    Table,
    table::{OptimizeAction, OptimizeOptions},
};
use miette::{IntoDiagnostic, Result};
use std::path::Path;
use tracing::{info, trace};

use crate::DEFAULT_CHUNKS_PATH_FIELD;

pub async fn optimize_index(table: &Table) -> Result<()> {
    table
        .optimize(OptimizeAction::Index(OptimizeOptions::default()))
        .await
        .into_diagnostic()?;
    Ok(())
}

pub async fn delete_by_path(table: &Table, path: &Path) -> Result<()> {
    if path.is_dir() {
        info!("Deleting all chunks for folder: {}", path.display());
        table
            .delete(&format!(
                r#"{} LIKE '{}%'"#,
                DEFAULT_CHUNKS_PATH_FIELD,
                path.to_string_lossy()
            ))
            .await
            .into_diagnostic()?;
        optimize_index(table).await?;
    } else {
        trace!("Deleting chunk for file: {}", path.display());
        table
            .delete(
                format!(
                    r#"{} = "{}""#,
                    DEFAULT_CHUNKS_PATH_FIELD,
                    path.to_string_lossy()
                )
                .as_str(),
            )
            .await
            .into_diagnostic()?;
    }
    Ok(())
}
