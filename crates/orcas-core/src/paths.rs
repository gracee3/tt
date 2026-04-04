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

    pub fn from_home(home_root: PathBuf) -> Self {
        Self::from_roots(
            home_root.clone(),
            home_root.clone(),
            home_root.join("runtime"),
        )
    }

    fn discover_from(home_dir: PathBuf, orcas_home: Option<PathBuf>) -> Self {
        let home_root = orcas_home.unwrap_or_else(|| home_dir.join(".orcas"));
        Self::from_home(home_root)
    }

    pub fn discover() -> OrcasResult<Self> {
        let home_dir = dirs::home_dir()
            .ok_or_else(|| OrcasError::Config("unable to resolve home directory".to_string()))?;
        Ok(Self::discover_from(
            home_dir,
            std::env::var_os("ORCAS_HOME").map(PathBuf::from),
        ))
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

    #[test]
    fn discover_from_defaults_to_single_hidden_home_root() {
        let paths = AppPaths::discover_from(PathBuf::from("/home/tester"), None);

        assert_eq!(paths.config_dir, PathBuf::from("/home/tester/.orcas"));
        assert_eq!(
            paths.config_file,
            PathBuf::from("/home/tester/.orcas/config.toml")
        );
        assert_eq!(paths.data_dir, PathBuf::from("/home/tester/.orcas"));
        assert_eq!(
            paths.state_file,
            PathBuf::from("/home/tester/.orcas/state.json")
        );
        assert_eq!(
            paths.state_db_file,
            PathBuf::from("/home/tester/.orcas/state.db")
        );
        assert_eq!(paths.logs_dir, PathBuf::from("/home/tester/.orcas/logs"));
        assert_eq!(
            paths.runtime_dir,
            PathBuf::from("/home/tester/.orcas/runtime")
        );
        assert_eq!(
            paths.socket_file,
            PathBuf::from("/home/tester/.orcas/runtime/orcasd.sock")
        );
    }

    #[test]
    fn discover_from_respects_orcas_home_override() {
        let paths = AppPaths::discover_from(
            PathBuf::from("/home/tester"),
            Some(PathBuf::from("/tmp/orcas-home")),
        );

        assert_eq!(paths.config_dir, PathBuf::from("/tmp/orcas-home"));
        assert_eq!(paths.data_dir, PathBuf::from("/tmp/orcas-home"));
        assert_eq!(paths.runtime_dir, PathBuf::from("/tmp/orcas-home/runtime"));
        assert_eq!(
            paths.socket_file,
            PathBuf::from("/tmp/orcas-home/runtime/orcasd.sock")
        );
    }
}
