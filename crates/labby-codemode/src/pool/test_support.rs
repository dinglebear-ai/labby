use super::*;

impl RunnerPool {
    pub(crate) fn with_test_spawn(config: PoolConfig, spawn: RunnerSpawn) -> Self {
        Self::with_config_and_spawn(config, spawn, None)
    }
}
