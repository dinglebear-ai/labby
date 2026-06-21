//! Configuration knob alias for the Code Mode warm-runner pool.
//!
//! The resolved/clamped pool configuration and its env knobs now live in the
//! centralized `crate::config` module ([`CodeModePoolConfig`]) so the same
//! definition feeds the runner pool and `gateway code status`/doctor without
//! drift (lab-xvmti). This module re-exports it under the pool's local name so
//! the pool code keeps reading `PoolConfig`.
//!
//! The kill switch (`LAB_CODE_MODE_POOL_SIZE=0` → spawn-per-execution) and the
//! clamp bounds are documented on [`CodeModePoolConfig`].

pub(in crate::dispatch::gateway::code_mode) use crate::config::CodeModePoolConfig as PoolConfig;
