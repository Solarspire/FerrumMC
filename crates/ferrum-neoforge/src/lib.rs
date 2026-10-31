use std::{
    fs,
    path::{Path, PathBuf},
};

use jni::{Env, InitArgsBuilder, JNIVersion, JavaVM, objects::JString};
use serde::Deserialize;

pub mod registry;
pub mod world;
pub mod worldgen;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("NeoForge JNI: {0}")]
    Jni(#[from] jni::errors::Error),
    #[error("NeoForge bridge I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("NeoForge bridge JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("NeoForge bridge: {0}")]
    Runtime(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Config {
    enabled: bool,
    launch: Option<PathBuf>,
    runtime: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Launch {
    vm_options: Vec<String>,
}

/// Owns bridge resources in the process's shared JVM.
pub struct Runtime {
    vm: JavaVM,
    report: serde_json::Value,
    pub registries: registry::Manifest,
}

impl Runtime {
    /// An absent configuration leaves the normal no-mod server path untouched.
    pub fn from_directory(directory: &Path) -> Result<Option<Self>, Error> {
        let path = directory.join("ferrum-neoforge.json");
        let config = match read_bounded(&path) {
            Ok(bytes) => serde_json::from_slice::<Config>(&bytes)?,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                if directory.join("ferrum-neoforge-registry.json").exists() {
                    return Err(Error::Runtime(
                        "this world requires its NeoForge runtime and registry manifest".into(),
                    ));
                }
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if !config.enabled {
            if directory.join("ferrum-neoforge-registry.json").exists() {
                return Err(Error::Runtime(
                    "this world requires its NeoForge runtime".into(),
                ));
            }
            return Ok(None);
        }
        let launch = config
            .launch
            .ok_or_else(|| Error::Runtime("enabled bridge requires launch".into()))?;
        let runtime = config
            .runtime
            .ok_or_else(|| Error::Runtime("enabled bridge requires runtime".into()))?;
        let runtime = Self::boot(&directory.join(launch), &directory.join(runtime))?;
        let marker = directory.join("ferrum-neoforge-registry.json");
        let names: std::collections::BTreeSet<_> = runtime
            .registries
            .states
            .iter()
            .map(|s| &s.key.name)
            .chain(runtime.registries.items.iter().map(|s| &s.name))
            .chain(runtime.registries.block_entity_types.iter())
            .filter(|s| !s.starts_with("minecraft:"))
            .cloned()
            .collect();
        if marker.exists() {
            let required: std::collections::BTreeSet<String> =
                serde_json::from_slice(&read_bounded(&marker)?)?;
            if let Some(missing) = required.difference(&names).next() {
                return Err(Error::Runtime(format!(
                    "world requires missing registry entry: {missing}"
                )));
            }
        }
        fs::write(marker, serde_json::to_vec(&names)?)?;
        Ok(Some(runtime))
    }

    /// Boots FML using an existing JVM or creates the process's only JVM.
    pub fn boot(launch: &Path, runtime: &Path) -> Result<Self, Error> {
        let launch: Launch = serde_json::from_slice(&read_bounded(launch)?)?;
        let runtime = runtime.canonicalize()?;
        let _ = read_bounded(&runtime)?;
        let mut builder = InitArgsBuilder::new().version(JNIVersion::V21);
        for option in launch.vm_options {
            builder = builder.option(option);
        }
        let args = builder
            .build()
            .map_err(|error| Error::Runtime(error.to_string()))?;
        // Never try to create a second JVM. An incompatible existing classpath fails visibly.
        let vm = match JavaVM::singleton() {
            Ok(vm) => vm,
            Err(_) => JavaVM::new(args).map_err(|error| Error::Runtime(error.to_string()))?,
        };
        let result = vm.attach_current_thread(|env| -> Result<serde_json::Value, Error> {
            let path = env
                .new_string(runtime.to_string_lossy())
                .map_err(Error::Jni)?;
            let boot = env.call_static_method(
                jni::jni_str!("net/solarspire/ferrum/runtime/BridgeRuntime"),
                jni::jni_str!("boot"),
                jni::jni_sig!("(Ljava/lang/String;)V"),
                &[(&path).into()],
            );
            checked(env, boot)?;
            let report = call_string(env, jni::jni_str!("dumpRegistries"))?;
            let report: serde_json::Value = serde_json::from_str(&report)?;
            if report["compatible"] != true || report["registriesFrozen"] != true {
                return Err(Error::Runtime(
                    "loader did not pass the frozen-registry compatibility gate".into(),
                ));
            }
            Ok(report)
        });
        let report = match result {
            Ok(report) => report,
            Err(error) => {
                let _ = shutdown_vm(&vm);
                return Err(error);
            }
        };
        let registries = vm.attach_current_thread(|env| {
            let json = call_string(env, jni::jni_str!("registryManifest"))?;
            registry::Manifest::parse(&json)
        });
        let registries = match registries {
            Ok(registries) => registries,
            Err(error) => {
                let _ = shutdown_vm(&vm);
                return Err(error);
            }
        };
        world::install(&vm, &registries)?;
        worldgen::install(&vm)?;
        tracing::info!(
            states = registries.states.len(),
            items = registries.items.len(),
            block_entity_types = registries.block_entity_types.len(),
            "NeoForge runtime attached; frozen registries ready"
        );
        Ok(Self {
            vm,
            report,
            registries,
        })
    }

    #[must_use]
    pub const fn report(&self) -> &serde_json::Value {
        &self.report
    }

    /// Returns names and property maps rather than interpreting `NeoForge` IDs as Pumpkin IDs.
    pub fn registry_manifest(&self) -> Result<String, Error> {
        self.vm
            .attach_current_thread(|env| call_string(env, jni::jni_str!("registryManifest")))
    }

    pub fn shutdown(self) -> Result<(), Error> {
        shutdown_vm(&self.vm)
    }
}

fn shutdown_vm(vm: &JavaVM) -> Result<(), Error> {
    vm.attach_current_thread(|env| {
        let result = env.call_static_method(
            jni::jni_str!("net/solarspire/ferrum/runtime/BridgeRuntime"),
            jni::jni_str!("shutdown"),
            jni::jni_sig!("()V"),
            &[],
        );
        checked(env, result).map(|_| ())
    })
}

fn checked<T>(env: &Env<'_>, result: jni::errors::Result<T>) -> Result<T, Error> {
    result.map_err(|error| {
        // Preserve the full Java cause chain before clearing the JNI exception.
        env.exception_describe();
        env.exception_clear();
        Error::Jni(error)
    })
}

fn call_string(env: &mut Env<'_>, method: &jni::strings::JNIStr) -> Result<String, Error> {
    let result = env.call_static_method(
        jni::jni_str!("net/solarspire/ferrum/runtime/BridgeRuntime"),
        method,
        jni::jni_sig!("()Ljava/lang/String;"),
        &[],
    );
    let object = checked(env, result)?.l().map_err(Error::Jni)?;
    let string = env.cast_local::<JString>(object).map_err(Error::Jni)?;
    string.try_to_string(env).map_err(Error::Jni)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, Error> {
    use std::io::Read;
    const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(Error::Runtime(format!("{} exceeds 1 MiB", path.display())));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{Error, Runtime};

    #[test]
    fn configuration_rejects_malformed_or_oversized_files_before_starting_jvm() -> Result<(), Error>
    {
        let directory = tempfile::tempdir()?;
        assert!(Runtime::from_directory(directory.path())?.is_none());
        let path = directory.path().join("ferrum-neoforge.json");
        std::fs::write(&path, br#"{"enabled":false}"#)?;
        assert!(Runtime::from_directory(directory.path())?.is_none());
        std::fs::write(&path, br#"{"enabled":true}"#)?;
        assert!(Runtime::from_directory(directory.path()).is_err());
        std::fs::write(&path, b"{")?;
        assert!(Runtime::from_directory(directory.path()).is_err());
        std::fs::write(&path, vec![b' '; 1024 * 1024 + 1])?;
        assert!(Runtime::from_directory(directory.path()).is_err());
        Ok(())
    }
}
