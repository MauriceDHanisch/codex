use std::sync::OnceLock;

const TOKIO_WORKER_THREADS_ENV_VAR: &str = "TOKIO_WORKER_THREADS";

/// Controls whether V8 may generate executable code at runtime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum V8JitMode {
    #[default]
    Enabled,
    Disabled,
}

struct V8Initialization {
    _platform: v8::SharedRef<v8::Platform>,
    jit_mode: V8JitMode,
}

static V8_INITIALIZATION: OnceLock<Result<V8Initialization, String>> = OnceLock::new();

/// Initializes the process-wide V8 platform with the requested JIT mode.
///
/// Call this before executing any code-mode cells when JIT must be disabled.
/// V8 cannot change JIT mode after initialization, so a later call requesting
/// a different mode returns an error. Code mode initializes V8 with JIT enabled
/// by default when this function has not been called explicitly.
pub fn initialize_v8(jit_mode: V8JitMode) -> Result<(), String> {
    match V8_INITIALIZATION.get_or_init(|| initialize_v8_with_mode(jit_mode)) {
        Ok(initialization) if initialization.jit_mode == jit_mode => Ok(()),
        Ok(initialization) => Err(format!(
            "V8 was already initialized with JIT {}",
            initialization.jit_mode.description()
        )),
        Err(error_text) => Err(error_text.clone()),
    }
}

pub(crate) fn ensure_v8_initialized() -> Result<(), String> {
    match V8_INITIALIZATION.get_or_init(|| initialize_v8_with_mode(V8JitMode::Enabled)) {
        Ok(_) => Ok(()),
        Err(error_text) => Err(error_text.clone()),
    }
}

fn initialize_v8_with_mode(jit_mode: V8JitMode) -> Result<V8Initialization, String> {
    v8::icu::set_common_data_77(deno_core_icudata::ICU_DATA)
        .map_err(|error_code| format!("failed to initialize ICU data: {error_code}"))?;
    match jit_mode {
        V8JitMode::Enabled => {}
        V8JitMode::Disabled => v8::V8::set_flags_from_string("--jitless"),
    }
    // Keep the V8 worker pool aligned with the main Codex runtime when a
    // process-wide worker limit is configured.
    let platform = v8::new_default_platform(
        v8_thread_pool_size_from_env(std::env::var(TOKIO_WORKER_THREADS_ENV_VAR).ok().as_deref())?,
        false,
    )
    .make_shared();
    v8::V8::initialize_platform(platform.clone());
    v8::V8::initialize();
    Ok(V8Initialization {
        _platform: platform,
        jit_mode,
    })
}

fn v8_thread_pool_size_from_env(value: Option<&str>) -> Result<u32, String> {
    let Some(value) = value else {
        return Ok(0);
    };
    let thread_count = value
        .parse::<u32>()
        .map_err(|_| format!("{TOKIO_WORKER_THREADS_ENV_VAR} must be a positive integer"))?;
    if thread_count == 0 {
        return Err(format!(
            "{TOKIO_WORKER_THREADS_ENV_VAR} must be a positive integer"
        ));
    }
    Ok(thread_count)
}

impl V8JitMode {
    fn description(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

#[cfg(test)]
#[path = "v8_init_tests.rs"]
mod tests;
