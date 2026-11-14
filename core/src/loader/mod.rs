pub mod neoforge;
pub mod fabric;
pub mod quilt;
pub mod forge;

use std::path::{Path, PathBuf};

use crate::types::Result;

/// Écriture atomique : fichier `.tmp` voisin puis renommage.
pub(crate) async fn write_atomic(dest: &Path, data: &[u8]) -> Result<()> {
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    tokio::fs::write(&tmp, data).await?;
    if let Err(e) = tokio::fs::rename(&tmp, dest).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    Ok(())
}
