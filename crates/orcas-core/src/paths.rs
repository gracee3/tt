use std::path::PathBuf;

use crate::error::{OrcasError, OrcasResult};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub state_file: PathBuf,
    pub state_db_file: PathBuf,
    pub logs_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub socket_file: PathBuf,
    pub daemon_metadata_file: PathBuf,
    pub daemon_log_file: PathBuf,
}

impl AppPaths {
    pub fn from_roots(config_dir: PathBuf, data_dir: PathBuf, runtime_dir: PathBuf) -> Self {
        let logs_dir = data_dir.join("logs");
        Self {
            config_file: config_dir.join("config.toml"),
            state_file: data_dir.join("state.json"),
            state_db_file: data_dir.join("state.db"),
            socket_file: runtime_dir.join("orcasd.sock"),
            daemon_metadata_file: runtime_dir.join("orcasd.json"),
            daemon_log_file: logs_dir.join("orcasd.log"),
            config_dir,
            data_dir,
            logs_dir,
            runtime_dir,
        }
    }

    pub fn discover() -> OrcasResult<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| OrcasError::Config("unable to resolve config directory".to_string()))?
            .join("orcas");
        let data_dir = dirs::data_dir()
            .ok_or_else(|| OrcasError::Config("unable to resolve data directory".to_string()))?
            .join("orcas");
        let runtime_dir = dirs::runtime_dir()
            .unwrap_or_else(|| data_dir.join("runtime"))
            .join("orcas");
        Ok(Self::from_roots(config_dir, data_dir, runtime_dir))
    }

    pub async fn ensure(&self) -> OrcasResult<()> {
        tokio::fs::create_dir_all(&self.config_dir).await?;
        tokio::fs::create_dir_all(&self.data_dir).await?;
        tokio::fs::create_dir_all(&self.logs_dir).await?;
        tokio::fs::create_dir_all(&self.runtime_dir).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::AppPaths;

    #[test]
    fn from_roots_keeps_json_and_db_state_paths_side_by_side() {
        let paths = AppPaths::from_roots(
            PathBuf::from("/tmp/orcas/config"),
            PathBuf::from("/tmp/orcas/data"),
            PathBuf::from("/tmp/orcas/runtime"),
        );

        assert_eq!(
            paths.state_file,
            PathBuf::from("/tmp/orcas/data/state.json")
        );
        assert_eq!(
            paths.state_db_file,
            PathBuf::from("/tmp/orcas/data/state.db")
        );
        assert_eq!(
            paths.daemon_log_file,
            PathBuf::from("/tmp/orcas/data/logs/orcasd.log")
        );
    }
}
