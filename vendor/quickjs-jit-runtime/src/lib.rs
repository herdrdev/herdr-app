#![cfg_attr(rquickjs_memory_sanitizer, feature(thread_local))]

//! Optional tiered JIT integration for `rquickjs`.
//!
//! The runtime attaches through a versioned engine ABI while execution remains
//! on the QuickJS interpreter until compiler tiers are enabled.

pub mod abi;
pub mod bytecode;
pub mod code_cache;
pub mod compiler;
mod config;
pub mod correctness;
mod error;
#[cfg(feature = "compiler")]
pub mod ir;
mod metrics;
pub mod platform;
pub mod runtime;

#[cfg(feature = "test-support")]
#[doc(hidden)]
#[path = "../tests/support/mod.rs"]
pub mod test_support;

use core::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub use config::{JitConfig, JitConfigBuilder, JitDiagnostic, JitDiagnosticKind, JitTierPolicy};
pub use error::JitError;
pub use metrics::JitMetrics;
pub use rquickjs_core::Runtime;

const NATIVE_EXECUTION_SUPPORTED: bool = cfg!(any(
    all(
        target_os = "macos",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(
        target_os = "windows",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(
        target_os = "linux",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
));

#[cfg(all(
    feature = "compiler",
    any(
        all(
            target_os = "macos",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "windows",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "linux",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )
))]
fn artifact_environment(
    runtime_id: u64,
    info: &abi::AbiInfo,
    config: &JitConfig,
    compiler: &compiler::baseline::BaselineCompiler,
) -> runtime::ArtifactEnvironment {
    fn mix(hash: u64, value: u64) -> u64 {
        (hash ^ value).wrapping_mul(0x100000001b3)
    }
    let target_identity = compiler.target_identity();
    let mut config_fingerprint = 0xcbf29ce484222325;
    for value in [
        u64::from(config.call_threshold()),
        u64::from(config.loop_threshold()),
        config.max_code_bytes() as u64,
        config.max_metadata_bytes() as u64,
        config.max_snapshot_bytes() as u64,
        config.max_ir_bytes() as u64,
        config.compile_timeout_ms(),
        config.max_queue_len() as u64,
        config.workers() as u64,
        u64::from(config.max_compile_attempts()),
        match config.tier_policy() {
            JitTierPolicy::Automatic => 0,
            JitTierPolicy::BaselineOnly => 1,
            JitTierPolicy::Optimize => 2,
        },
    ] {
        config_fingerprint = mix(config_fingerprint, value);
    }
    runtime::ArtifactEnvironment {
        runtime_id,
        target_isa: target_identity.triple_fingerprint(),
        cpu_features: target_identity.codegen_fingerprint(),
        abi_fingerprint: mix(
            mix(info.build_fingerprint(), info.source_revision()),
            info.opcode_fingerprint(),
        ),
        config_fingerprint,
    }
}

/// Backend-owned ABI 1.25 fast-entry state shared with QuickJS.
///
/// QuickJS reads `epoch` and increments the counters with plain stores on
/// the runtime thread while executing callback-free native calls. Rust only
/// loads them, so a reader on another thread may observe a stale count. The
/// allocation outlives the attached backend, which keeps one `Arc` clone.
#[repr(C)]
#[derive(Debug)]
struct FastEntryShared {
    struct_size: u32,
    reserved: u32,
    epoch: AtomicU64,
    entries: AtomicU64,
    exits: AtomicU64,
    optimized_entries: AtomicU64,
}

const _: () = {
    use core::mem::{align_of, offset_of, size_of};
    use rquickjs_core::qjs::JSJitFastEntryState as Raw;
    assert!(size_of::<FastEntryShared>() == size_of::<Raw>());
    assert!(align_of::<FastEntryShared>() >= align_of::<Raw>());
    assert!(offset_of!(FastEntryShared, epoch) == offset_of!(Raw, epoch));
    assert!(offset_of!(FastEntryShared, entries) == offset_of!(Raw, entries));
    assert!(offset_of!(FastEntryShared, exits) == offset_of!(Raw, exits));
    assert!(offset_of!(FastEntryShared, optimized_entries) == offset_of!(Raw, optimized_entries));
};

impl FastEntryShared {
    fn new(epoch: u64) -> Self {
        Self {
            struct_size: core::mem::size_of::<rquickjs_core::qjs::JSJitFastEntryState>() as u32,
            reserved: 0,
            epoch: AtomicU64::new(epoch),
            entries: AtomicU64::new(0),
            exits: AtomicU64::new(0),
            optimized_entries: AtomicU64::new(0),
        }
    }

    /// The counters and epoch are atomics, so QuickJS may update them through
    /// this pointer while Rust holds shared references.
    /// Unused by targets without the native production backend.
    #[allow(dead_code)]
    fn as_raw(&self) -> *mut rquickjs_core::qjs::JSJitFastEntryState {
        (self as *const Self).cast_mut().cast()
    }

    /// Adds callback-free executions to a published snapshot.
    fn add_to(&self, metrics: &mut JitMetrics) {
        let entries = self.entries.load(Ordering::Relaxed);
        let exits = self.exits.load(Ordering::Relaxed);
        let optimized = self.optimized_entries.load(Ordering::Relaxed);
        metrics.native_entries = metrics.native_entries.saturating_add(entries);
        metrics.native_exits = metrics.native_exits.saturating_add(exits);
        metrics.tier2_entries = metrics.tier2_entries.saturating_add(optimized);
    }
}

/// Owns the guard that keeps a JIT backend attached to a runtime.
#[derive(Debug)]
pub struct Jit {
    metrics: Arc<Mutex<JitMetrics>>,
    fast_entry: Arc<FastEntryShared>,
    config: JitConfig,
    _guard: rquickjs_core::runtime::RuntimeJitGuard,
    #[cfg(all(feature = "test-support", feature = "compiler"))]
    test_runtime_id: u64,
    #[cfg(feature = "test-support")]
    test_environment: Arc<Mutex<Option<runtime::ArtifactEnvironment>>>,
    #[cfg(feature = "test-support")]
    test_last_acquired_key: Arc<Mutex<Option<code_cache::ArtifactKey>>>,
    #[cfg(feature = "test-support")]
    test_completion_dispositions:
        Arc<Mutex<std::collections::HashMap<runtime::FunctionKey, runtime::CompletionDisposition>>>,
}

impl Jit {
    /// Attaches the initial no-op backend to an existing runtime.
    pub fn attach(runtime: &Runtime, config: JitConfig) -> Result<Self, JitError> {
        let info = abi::AbiInfo::query_linked()?;
        Self::attach_with_info(runtime, config, info)
    }

    fn attach_with_info(
        runtime: &Runtime,
        config: JitConfig,
        info: abi::AbiInfo,
    ) -> Result<Self, JitError> {
        if let Err(error) = info.validate() {
            if let abi::AbiError::Incompatible(mismatch) = error {
                config.report(JitDiagnosticKind::AbiMismatch(mismatch));
            }
            return Err(error.into());
        }
        // Measure the interpreter's per-call stack cost once per process,
        // outside JIT callbacks, before any native call entry is compiled.
        #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
        let _ = compiler::native_call::interpreter_frame_bytes();

        let metrics = Arc::new(Mutex::new(JitMetrics::disabled()));
        #[cfg(all(
            feature = "compiler",
            any(
                all(
                    target_os = "macos",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "windows",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "linux",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                )
            )
        ))]
        let backend = ProductionBackend::new(
            runtime.jit_runtime_id(),
            &info,
            config.clone(),
            Arc::clone(&metrics),
        )?;
        #[cfg(all(
            feature = "test-support",
            feature = "compiler",
            any(
                all(
                    target_os = "macos",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "windows",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "linux",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                )
            )
        ))]
        let test_environment = Arc::clone(&backend.environment);
        #[cfg(all(
            feature = "test-support",
            feature = "compiler",
            any(
                all(
                    target_os = "macos",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "windows",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "linux",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                )
            )
        ))]
        let test_last_acquired_key = Arc::clone(&backend.test_last_acquired_key);
        #[cfg(all(
            feature = "test-support",
            feature = "compiler",
            any(
                all(
                    target_os = "macos",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "windows",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "linux",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                )
            )
        ))]
        let test_completion_dispositions = backend.coordinator.test_completion_dispositions();
        #[cfg(not(all(
            feature = "compiler",
            any(
                all(
                    target_os = "macos",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "windows",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "linux",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                )
            )
        )))]
        let backend = NoopBackend {
            _config: config.clone(),
        };
        #[cfg(not(all(
            feature = "compiler",
            any(
                all(
                    target_os = "macos",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "windows",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "linux",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                )
            )
        )))]
        let fast_entry = Arc::new(FastEntryShared::new(0));
        #[cfg(all(
            feature = "compiler",
            any(
                all(
                    target_os = "macos",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "windows",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "linux",
                    target_endian = "little",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                )
            )
        ))]
        let fast_entry = Arc::clone(&backend.fast_entry);
        let guard = match runtime.attach_jit_backend(backend) {
            Ok(guard) => guard,
            Err(error) => {
                config.report(JitDiagnosticKind::BackendAttachment);
                return Err(error.into());
            }
        };
        Ok(Self {
            metrics,
            fast_entry,
            config,
            _guard: guard,
            #[cfg(all(feature = "test-support", feature = "compiler"))]
            test_runtime_id: runtime.jit_runtime_id(),
            #[cfg(feature = "test-support")]
            test_environment: {
                #[cfg(all(
                    feature = "compiler",
                    any(
                        all(
                            target_os = "macos",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "windows",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "linux",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        )
                    )
                ))]
                {
                    test_environment
                }
                #[cfg(not(all(
                    feature = "compiler",
                    any(
                        all(
                            target_os = "macos",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "windows",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "linux",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        )
                    )
                )))]
                {
                    Arc::new(Mutex::new(None))
                }
            },
            #[cfg(feature = "test-support")]
            test_last_acquired_key: {
                #[cfg(all(
                    feature = "compiler",
                    any(
                        all(
                            target_os = "macos",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "windows",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "linux",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        )
                    )
                ))]
                {
                    test_last_acquired_key
                }
                #[cfg(not(all(
                    feature = "compiler",
                    any(
                        all(
                            target_os = "macos",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "windows",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "linux",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        )
                    )
                )))]
                {
                    Arc::new(Mutex::new(None))
                }
            },
            #[cfg(feature = "test-support")]
            test_completion_dispositions: {
                #[cfg(all(
                    feature = "compiler",
                    any(
                        all(
                            target_os = "macos",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "windows",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "linux",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        )
                    )
                ))]
                {
                    test_completion_dispositions
                }
                #[cfg(not(all(
                    feature = "compiler",
                    any(
                        all(
                            target_os = "macos",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "windows",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "linux",
                            target_endian = "little",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        )
                    )
                )))]
                {
                    Arc::new(Mutex::new(std::collections::HashMap::new()))
                }
            },
        })
    }

    /// Fails when the current target cannot support native execution.
    pub fn require_native(&self) -> Result<(), JitError> {
        if NATIVE_EXECUTION_SUPPORTED {
            Ok(())
        } else {
            Err(JitError::UnsupportedPlatform)
        }
    }

    /// Suspends hot, feedback, native-entry acquisition, and OSR probes in O(1).
    pub fn suspend(&self) -> Result<(), JitError> {
        self._guard.suspend().map_err(Into::into)
    }

    /// Resumes probes without tearing down the backend or installed artifacts.
    pub fn resume(&self) -> Result<(), JitError> {
        self._guard.resume().map_err(Into::into)
    }

    /// Returns whether this runtime's JIT probes are suspended.
    pub fn is_suspended(&self) -> bool {
        self._guard.is_suspended()
    }

    /// Returns the metrics associated with this backend guard.
    pub fn metrics(&self) -> JitMetrics {
        let mut snapshot = self
            .metrics
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        self.fast_entry.add_to(&mut snapshot);
        snapshot
    }

    /// Performs bounded installation and reclamation work on the runtime thread.
    pub fn poll(&self) {
        self._guard.poll();
        let snapshot = self.metrics();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.config.observe(&snapshot);
        }));
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn test_artifact_environment(&self) -> runtime::ArtifactEnvironment {
        self.test_environment
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .expect("production compiler backend has not received eligible work")
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn test_artifact_environment_initialized(&self) -> bool {
        self.test_environment
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn test_last_acquired_artifact_key(&self) -> Option<code_cache::ArtifactKey> {
        *self
            .test_last_acquired_key
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn test_completion_dispositions(
        &self,
    ) -> Vec<(runtime::FunctionKey, runtime::CompletionDisposition)> {
        let mut dispositions = self
            .test_completion_dispositions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(key, disposition)| (*key, *disposition))
            .collect::<Vec<_>>();
        dispositions.sort_unstable_by_key(|(key, _)| (key.id, key.generation));
        dispositions
    }

    #[cfg(all(feature = "test-support", feature = "compiler"))]
    #[doc(hidden)]
    pub fn test_tier2_compile_dispositions(
        &self,
    ) -> Vec<(
        runtime::FunctionKey,
        compiler::optimized::Tier2CompileDisposition,
    )> {
        compiler::optimized::test_tier2_compile_dispositions(self.test_runtime_id)
    }

    #[cfg(all(feature = "test-support", feature = "compiler"))]
    #[doc(hidden)]
    pub fn test_tier2_deopt_sites(&self) -> Vec<((runtime::FunctionKey, u32), (u32, u8))> {
        compiler::optimized::test_tier2_deopt_sites(self.test_runtime_id)
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn test_last_deopt_guards(&self) -> Vec<(runtime::FunctionKey, u32)> {
        let mut guards = test_deopt_guards()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(key, guard)| (*key, *guard))
            .collect::<Vec<_>>();
        guards.sort_unstable_by_key(|(key, _)| (key.id, key.generation));
        guards
    }
}

#[cfg(not(all(
    feature = "compiler",
    any(
        all(
            target_os = "macos",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "windows",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "linux",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )
)))]
#[derive(Debug)]
struct NoopBackend {
    _config: JitConfig,
}

#[cfg(not(all(
    feature = "compiler",
    any(
        all(
            target_os = "macos",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "windows",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "linux",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )
)))]
unsafe impl rquickjs_core::runtime::JitBackend for NoopBackend {}

/// Upper bound on consecutive per-call ticks that may skip the full
/// maintenance pass when no completion, feedback change, or installation
/// happened. Backoff windows are measured in clock ticks, which still advance
/// on every call, so this only bounds the latency of candidate scans.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const HOT_MAINTENANCE_INTERVAL: u32 = 64;

/// Inputs of a deferred Tier-2 candidate decision: feedback lattices, the
/// installed-artifact count (callee resolution and direct-call readiness), and
/// whether the element-loop gate has its eight baseline samples.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Tier2ScanInputs {
    feedback: u64,
    shapes: u64,
    installed: u64,
    element_samples: u64,
}

/// Callback-free executions granted per full call of a steady native entry.
/// Every renewal still runs hot, feedback, timing and maintenance callbacks.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const FAST_ENTRY_BUDGET: u32 = 255;

/// Baseline executions an untranslatable generation serves before it is
/// settled into the interpreter (see `settle_untranslatable_tier2_candidate`).
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const UNTRANSLATABLE_SETTLE_EXECUTIONS: u64 = 16;

/// Opcodes that Tier 1 admitted only through the P2b slice (82d3808 rejected
/// each of them). A function containing one ran in the interpreter at
/// 82d3808; the untranslatable settle returns it there. Every other function
/// keeps the unchanged optimizing scan, so Tier 1-only code that already ran
/// natively (string/object literals, `new`, ...) keeps its baseline.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const SETTLED_GENERIC_TIER1_OPCODES: &[&str] = &[
    "push_this",
    "special_object",
    "dup1",
    "dup2",
    "perm4",
    "swap2",
    "rot3l",
    "rot3r",
    "get_var_undef",
    "put_var",
    "to_object",
    "to_propkey2",
    "typeof",
    "delete",
    "delete_var",
    "pow",
    "instanceof",
    "in",
    "typeof_is_undefined",
    "typeof_is_function",
];

/// Whether the optimizing scan settles this generation into the interpreter
/// instead of evaluating a Tier 2 trial: Tier 2 can never translate it and
/// it contains an opcode from `SETTLED_GENERIC_TIER1_OPCODES`.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn settles_untranslatable_candidate(snapshot: &bytecode::VerifiedFunction) -> bool {
    snapshot
        .instructions()
        .iter()
        .any(|instruction| SETTLED_GENERIC_TIER1_OPCODES.contains(&instruction.opcode().name()))
        && !ir::optimized_opcodes_supported(snapshot)
}

/// Feedback pc used for return types observed at native `DONE` exits.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const NATIVE_RETURN_FEEDBACK_PC: u32 = u32::MAX;

#[cfg(all(
    feature = "compiler",
    any(
        all(
            target_os = "macos",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "windows",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "linux",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )
))]
struct ProductionBackend {
    runtime_id: u64,
    abi_info: abi::AbiInfo,
    environment: Arc<Mutex<Option<runtime::ArtifactEnvironment>>>,
    config: JitConfig,
    coordinator: runtime::Coordinator,
    workers: runtime::BackgroundCompiler,
    requested: std::collections::HashSet<runtime::FunctionKey>,
    // Hot identity tables use VM-assigned keys, never guest-provided names.
    hotness: rustc_hash::FxHashMap<runtime::FunctionKey, runtime::HotnessState>,
    optimizing_requested: std::collections::HashSet<runtime::FunctionKey>,
    optimizing_hotness: rustc_hash::FxHashMap<runtime::FunctionKey, runtime::HotnessState>,
    optimizing_snapshots:
        std::collections::HashMap<runtime::FunctionKey, bytecode::VerifiedFunction>,
    baseline_property_refreshed: std::collections::HashSet<runtime::FunctionKey>,
    // Cached feedback-independent Tier-2 vocabulary admission per function.
    optimized_vocabulary: std::collections::HashMap<runtime::FunctionKey, bool>,
    tier2_sources: std::collections::HashMap<runtime::FunctionKey, bytecode::VerifiedFunction>,
    feedback: runtime::FeedbackTable,
    // CALL feedback consumers finish synchronously and never reenter QuickJS.
    call_feedback_types: Vec<runtime::ObservedType>,
    shape_feedback: runtime::ShapeFeedbackTable,
    metrics: Arc<Mutex<JitMetrics>>,
    cold_metrics_dirty: bool,
    native_entries: u64,
    native_acquisitions: u64,
    native_exits: u64,
    native_fallbacks: u64,
    native_retries: u64,
    osr_entries: u64,
    osr_not_ready: u64,
    osr_map_misses: u64,
    osr_validation_failures: u64,
    osr_attempts: u64,
    osr_generated_retries: u64,
    osr_validation: Arc<OsrValidationMetrics>,
    clock: u64,
    /// Per-call runtime callbacks (entry acquisition, hot probes, exits) run
    /// the full maintenance pass only when it can change anything: pending
    /// worker completions, new feedback, or a bounded number of skipped
    /// ticks. The clock still advances on every tick.
    hot_ticks_since_maintenance: u32,
    generic_call_rejections: u64,
    last_scan_feedback_version: u64,
    last_scan_installed: u64,
    last_refresh_scan: Option<(u64, u64)>,
    /// Tier-2 candidates whose last scan deferred them for missing feedback or
    /// an unresolved callee, keyed by the inputs that decision depended on.
    /// Rescanning with equal inputs would repeat the same feedback snapshot
    /// and reach the same deferral, so maintenance skips it.
    tier2_deferred: rustc_hash::FxHashMap<runtime::FunctionKey, Tier2ScanInputs>,
    direct_refresh_probes: std::collections::HashMap<runtime::FunctionKey, (u64, u64)>,
    queue_reasons: std::collections::HashMap<runtime::FunctionKey, runtime::HotReason>,
    prequeue_backoff: std::collections::HashMap<runtime::FunctionKey, (u8, u64)>,
    hot_call_queues: u64,
    hot_loop_queues: u64,
    adaptive_neutral_queues: u64,
    adaptive_inputs_recorded: u64,
    snapshot_requests: u64,
    stable_path_compile_requests: u64,
    // Cached C handles can execute repeatedly without another acquisition.
    // Replacement acquisition updates this before native_enter; active frames
    // retain their own tier in execution_starts across recursive replacement.
    entry_tiers: rustc_hash::FxHashMap<runtime::FunctionKey, runtime::Tier>,
    entry_cache_epoch: u64,
    /// ABI 1.25 state granted to cached steady-state entries. Its epoch
    /// mirrors `entry_cache_epoch`.
    fast_entry: Arc<FastEntryShared>,
    // C brackets each native invocation with a synchronous enter/exit pair.
    // Keep active records through retirement and preserve each invocation's tier.
    // The flag marks an OSR entry, which times only the rest of one call.
    execution_starts: Vec<(
        runtime::FunctionKey,
        std::time::Instant,
        runtime::Tier,
        bool,
    )>,
    execution_profiles: rustc_hash::FxHashMap<runtime::FunctionKey, ProductionProfile>,
    profitability_evaluations: u64,
    profitability_approved: u64,
    profitability_rejected: u64,
    profitability_backoff: std::collections::HashMap<runtime::FunctionKey, (u8, u64)>,
    /// Earliest future `profitability_backoff` retry clock (`u64::MAX` when
    /// none). Retries are evaluated by maintenance, so reaching one makes
    /// maintenance due; otherwise quiet native code would stretch each short
    /// backoff to a full `HOT_MAINTENANCE_INTERVAL`.
    next_profitability_retry: u64,
    /// Baseline was measured harmful and unpublished.  This is deliberately
    /// not a terminal function blacklist: stable feedback may still justify
    /// one of the coordinator's bounded optimizing-tier trials.
    profitability_blacklisted: rustc_hash::FxHashSet<runtime::FunctionKey>,
    /// Generations whose bytecode Tier 2 can never translate and whose
    /// baseline decision is settled; they leave the optimizing scan.
    tier2_untranslatable: rustc_hash::FxHashSet<runtime::FunctionKey>,
    /// Functions whose Tier 2 artifact runs self recursion as native calls.
    /// One optimized entry then covers a whole recursion tree, so its
    /// invocation time is not comparable with per-call baseline samples.
    native_recursive: rustc_hash::FxHashSet<runtime::FunctionKey>,
    /// Last native-entry admission answer per function and signature.
    native_call_plans: rustc_hash::FxHashMap<
        runtime::FunctionKey,
        (
            Box<[runtime::FeedbackRepresentation]>,
            runtime::FeedbackRepresentation,
            bool,
        ),
    >,
    /// Immutable generations for which neither tier can ever produce code.
    /// `record_hot` reports these to QuickJS so it can turn off all probes and
    /// feedback at the bytecode object, avoiding a permanent C -> Rust tax.
    feedback_disabled: rustc_hash::FxHashSet<runtime::FunctionKey>,
    benefit_recordings: u64,
    measured_benefit_ns: u64,
    compiler_measurements: Arc<CompilerMeasurements>,
    install_ns: u64,
    peak_compiler_bytes: usize,
    #[cfg(feature = "test-support")]
    test_last_acquired_key: Arc<Mutex<Option<code_cache::ArtifactKey>>>,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[derive(Default)]
struct CompilerMeasurements {
    elapsed_ns: AtomicU64,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
struct MeasuredCompiler<C> {
    inner: C,
    measurements: Arc<CompilerMeasurements>,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[derive(Default)]
struct DeferredTieredCompiler {
    compiler: std::sync::OnceLock<compiler::optimized::TieredCompiler>,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
impl compiler::Compiler for DeferredTieredCompiler {
    fn compile(
        &self,
        request: runtime::CompileRequest,
    ) -> Result<code_cache::CompiledArtifact, compiler::CompileFailure> {
        self.compiler
            .get_or_init(compiler::optimized::TieredCompiler::host)
            .compile(request)
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
impl<C: compiler::Compiler> compiler::Compiler for MeasuredCompiler<C> {
    fn compile(
        &self,
        request: runtime::CompileRequest,
    ) -> Result<code_cache::CompiledArtifact, compiler::CompileFailure> {
        let started = std::time::Instant::now();
        let result = self.inner.compile(request);
        self.measurements.elapsed_ns.fetch_add(
            started.elapsed().as_nanos().try_into().unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        result
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[derive(Clone, Copy, Debug, Default)]
struct ProductionProfile {
    bytecodes: u64,
    helper_calls: u64,
    baseline_executions: u64,
    baseline_ns: u64,
    /// Fastest complete Baseline invocation (entered at its first
    /// instruction, not through OSR); zero until one is observed.
    /// Preemption only lengthens a sample, so the minimum still shows whether
    /// entry cost can dominate the body.
    baseline_min_ns: u64,
    /// Baseline entries not nested in an active execution of the same
    /// function: `baseline_executions / baseline_outermost` estimates how many
    /// calls one outside entry performs through self recursion.
    baseline_outermost: u64,
    optimized_executions: u64,
    optimized_ns: u64,
    /// Fastest optimized invocation observed so far; meaningful only while
    /// `optimized_executions > 0`.
    optimized_min_ns: u64,
    tier2_trial_decided: bool,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
impl ProductionProfile {
    fn record_baseline(&mut self, elapsed_ns: u64, complete: bool) {
        if complete {
            let elapsed_ns = elapsed_ns.max(1);
            self.baseline_min_ns = match self.baseline_min_ns {
                0 => elapsed_ns,
                fastest => fastest.min(elapsed_ns),
            };
        }
        self.baseline_executions = self.baseline_executions.saturating_add(1);
        self.baseline_ns = self.baseline_ns.saturating_add(elapsed_ns);
    }

    fn record_optimized(&mut self, elapsed_ns: u64) {
        self.optimized_min_ns = if self.optimized_executions == 0 {
            elapsed_ns
        } else {
            self.optimized_min_ns.min(elapsed_ns)
        };
        self.optimized_executions = self.optimized_executions.saturating_add(1);
        self.optimized_ns = self.optimized_ns.saturating_add(elapsed_ns);
    }
}

/// Automatic-tiering profitability gate for closure-creating functions.
///
/// Tier 1 lowers `fclosure`, var-ref access and `close_loc` through the
/// generic helper bridge, and a function that creates closures must also
/// reload every argument and local after each helper. Tier 2 rejects these
/// opcodes, so such a function would stay in Tier 1 for its lifetime with
/// no optimizing follow-up. Measured on `calls-recursion-closures`, that
/// Tier 1 loop runs at about 0.74x the interpreter's speed. Until the
/// bridge (roadmap P1/P3) or Tier 2 var refs make it profitable, automatic
/// tiering leaves closure-creating functions in the interpreter. Explicit
/// tier policies (`BaselineOnly`, `Optimize`) and forced test tiers still
/// compile them, so the lowering stays covered.
#[cfg(all(
    feature = "compiler",
    any(
        all(
            target_os = "macos",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "windows",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "linux",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )
))]
fn automatic_closure_creation_unprofitable(
    config: &JitConfig,
    verified: &bytecode::VerifiedFunction,
) -> bool {
    #[cfg(feature = "test-support")]
    if config.force_optimized() {
        return false;
    }
    config.tier_policy() == JitTierPolicy::Automatic
        && verified
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction.opcode().name(), "fclosure" | "fclosure8"))
}

/// Opcodes Tier 1 has compiled since roadmap P2, each with the reason
/// automatic tiering rejected it at 0.12.9. Their helper-bridged Baselines
/// are shorter than native entry bookkeeping in host UI code (gpui-shell's
/// panel builder ran 0.8x the interpreter's speed) and settle through extra
/// compiles that delay hot kernels. Until profitability measures the
/// interpreter, automatic tiering keeps such functions interpreted; explicit
/// tier policies and forced test tiers still compile them.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const AUTOMATIC_DEFERRED_TIER1_OPCODES: &[(&str, bytecode::FallbackReason)] = &[
    ("fclosure", bytecode::FallbackReason::UnsupportedOpcode),
    ("push_this", bytecode::FallbackReason::ExtendedFrame),
    (
        "special_object",
        bytecode::FallbackReason::UnsupportedOpcode,
    ),
    ("dup1", bytecode::FallbackReason::UnsupportedOpcode),
    ("dup2", bytecode::FallbackReason::UnsupportedOpcode),
    ("perm4", bytecode::FallbackReason::UnsupportedOpcode),
    ("swap2", bytecode::FallbackReason::UnsupportedOpcode),
    ("rot3l", bytecode::FallbackReason::UnsupportedOpcode),
    ("rot3r", bytecode::FallbackReason::UnsupportedOpcode),
    ("throw", bytecode::FallbackReason::UnsupportedOpcode),
    ("throw_error", bytecode::FallbackReason::UnsupportedOpcode),
    ("get_var_undef", bytecode::FallbackReason::UnsupportedOpcode),
    ("put_var", bytecode::FallbackReason::UnsupportedOpcode),
    ("set_name", bytecode::FallbackReason::ExtendedFrame),
    ("get_var_ref", bytecode::FallbackReason::ClosureFrame),
    ("put_var_ref", bytecode::FallbackReason::ClosureFrame),
    ("set_var_ref", bytecode::FallbackReason::ClosureFrame),
    ("get_var_ref_check", bytecode::FallbackReason::ClosureFrame),
    ("put_var_ref_check", bytecode::FallbackReason::ClosureFrame),
    ("close_loc", bytecode::FallbackReason::UnsupportedOpcode),
    ("catch", bytecode::FallbackReason::ExceptionRegion),
    ("gosub", bytecode::FallbackReason::ExceptionRegion),
    ("ret", bytecode::FallbackReason::ExceptionRegion),
    ("nip_catch", bytecode::FallbackReason::ExceptionRegion),
    ("to_object", bytecode::FallbackReason::UnsupportedOpcode),
    ("to_propkey2", bytecode::FallbackReason::UnsupportedOpcode),
    ("for_in_start", bytecode::FallbackReason::UnsupportedOpcode),
    ("for_of_start", bytecode::FallbackReason::UnsupportedOpcode),
    ("for_in_next", bytecode::FallbackReason::UnsupportedOpcode),
    ("for_of_next", bytecode::FallbackReason::UnsupportedOpcode),
    (
        "iterator_close",
        bytecode::FallbackReason::UnsupportedOpcode,
    ),
    ("typeof", bytecode::FallbackReason::UnsupportedOpcode),
    ("delete", bytecode::FallbackReason::UnsupportedOpcode),
    ("delete_var", bytecode::FallbackReason::UnsupportedOpcode),
    ("pow", bytecode::FallbackReason::UnsupportedOpcode),
    ("instanceof", bytecode::FallbackReason::UnsupportedOpcode),
    ("in", bytecode::FallbackReason::UnsupportedOpcode),
    ("fclosure8", bytecode::FallbackReason::UnsupportedOpcode),
    ("get_var_ref0", bytecode::FallbackReason::ClosureFrame),
    ("get_var_ref1", bytecode::FallbackReason::ClosureFrame),
    ("get_var_ref2", bytecode::FallbackReason::ClosureFrame),
    ("get_var_ref3", bytecode::FallbackReason::ClosureFrame),
    ("put_var_ref0", bytecode::FallbackReason::ClosureFrame),
    ("put_var_ref1", bytecode::FallbackReason::ClosureFrame),
    ("put_var_ref2", bytecode::FallbackReason::ClosureFrame),
    ("put_var_ref3", bytecode::FallbackReason::ClosureFrame),
    ("set_var_ref0", bytecode::FallbackReason::ClosureFrame),
    ("set_var_ref1", bytecode::FallbackReason::ClosureFrame),
    ("set_var_ref2", bytecode::FallbackReason::ClosureFrame),
    ("set_var_ref3", bytecode::FallbackReason::ClosureFrame),
    (
        "typeof_is_undefined",
        bytecode::FallbackReason::UnsupportedOpcode,
    ),
    (
        "typeof_is_function",
        bytecode::FallbackReason::UnsupportedOpcode,
    ),
];

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn automatic_deferred_tier1_rejection(
    config: &JitConfig,
    verified: &bytecode::VerifiedFunction,
) -> Option<bytecode::FallbackReason> {
    #[cfg(feature = "test-support")]
    if config.force_optimized() {
        return None;
    }
    if config.tier_policy() != JitTierPolicy::Automatic {
        return None;
    }
    verified.instructions().iter().find_map(|instruction| {
        let name = instruction.opcode().name();
        AUTOMATIC_DEFERRED_TIER1_OPCODES
            .iter()
            .find(|(deferred, _)| *deferred == name)
            .map(|(_, reason)| *reason)
    })
}

/// Fastest Baseline execution below which exhausted Tier2 profitability
/// retries still return a function to the interpreter. Native entry and exit
/// bookkeeping (including two `Instant` reads) costs a fraction of a
/// microsecond, so it can outweigh a body as short as `return a + b` (about
/// 0.5 us per Baseline execution, bookkeeping included). A Baseline that runs
/// for longer cannot lose to the interpreter through entry cost; it keeps
/// running while its bounded Tier2 trial waits for the compile queue.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const ENTRY_DOMINATED_BASELINE_NS: u64 = 10_000;

/// Minimum observed self-recursive calls per outside entry before a pure
/// recursive function is admitted to the native-call convention.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const NATIVE_RECURSION_MIN_CALLS_PER_ENTRY: u64 = 16;
/// Baseline executions observed before that ratio can admit a function.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const NATIVE_RECURSION_PROFILE_EXECUTIONS: u64 = 64;
/// Outside entries observed before a shallow ratio rejects a function.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const NATIVE_RECURSION_PROFILE_ENTRIES: u64 = 64;

/// Finish the bounded Tier-2 profitability trial once. Production JITs patch
/// an IC/tier state after classification; repeating wide average comparisons
/// on every successful optimized exit would turn policy into hot-path tax.
/// `Some(true)` requests tier-down, `Some(false)` records a profitable trial.
///
/// Tier-2 loses only when even its fastest invocation in the window is 25%
/// slower than the baseline average. Sub-microsecond callees are timed through
/// callbacks whose own cost rivals the margin, so one preempted or cold sample
/// among eight must not decide the trial by itself.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn classify_tier2_trial(profile: &mut ProductionProfile) -> Option<bool> {
    if profile.tier2_trial_decided
        || profile.optimized_executions < 8
        || profile.baseline_executions == 0
    {
        return None;
    }
    let loss = u128::from(profile.optimized_min_ns)
        .saturating_mul(u128::from(profile.baseline_executions))
        .saturating_mul(4)
        >= u128::from(profile.baseline_ns).saturating_mul(5);
    profile.tier2_trial_decided = true;
    Some(loss)
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
struct ProductionEntryPin {
    execution: code_cache::ExecutionPin,
    native: *const u8,
    runtime_id: u64,
    key: runtime::FunctionKey,
    pc: u32,
    stack_map_count: u32,
    osr: Option<runtime::OsrMap>,
    /// Immutable argument/local buffer dimensions, computed when the entry is
    /// acquired. Like V8/JSC entry metadata, this keeps validation metadata
    /// discovery out of the per-invocation trampoline hot path.
    frame_counts: Option<(usize, usize)>,
    validation: Arc<OsrValidationMetrics>,
    #[cfg(feature = "test-support")]
    stress_gc: bool,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
type PendingDeoptGuards = std::collections::HashMap<
    (u64, u64),
    std::collections::VecDeque<(u32, Option<runtime::ObservedType>)>,
>;
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
struct PendingNativeReturn {
    function: runtime::FunctionKey,
    observed: runtime::ObservedType,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[derive(Default)]
struct OsrValidationMetrics {
    successes: AtomicU64,
    failures: AtomicU64,
    validation_retries: Mutex<std::collections::HashMap<(u64, u64, u32), u64>>,
    deopt_guards: Mutex<PendingDeoptGuards>,
    deopt_materializations: AtomicU64,
    side_path_entries: AtomicU64,
    /// The trampoline publishes after the native call has returned; C invokes
    /// native_exit immediately afterward, before any release callback or other
    /// JavaScript reentry. Even recursion therefore has only one pending return
    /// per runtime. Keep this handoff inline instead of allocating a queue for
    /// each call. The interpreter records return feedback at `OP_return`, but
    /// a function that returns natively (including a first invocation that
    /// entered baseline code through OSR) never executes that opcode, and a
    /// call signature that never completes would block Tier 2 forever.
    native_return: Mutex<Option<PendingNativeReturn>>,
}

#[cfg(feature = "test-support")]
fn test_deopt_guards() -> &'static Mutex<std::collections::HashMap<runtime::FunctionKey, u32>> {
    static GUARDS: std::sync::OnceLock<
        Mutex<std::collections::HashMap<runtime::FunctionKey, u32>>,
    > = std::sync::OnceLock::new();
    GUARDS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
impl OsrValidationMetrics {
    fn mark_validation_retry(&self, id: u64, generation: u64, pc: u32) {
        let mut retries = self
            .validation_retries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let pending = retries.entry((id, generation, pc)).or_default();
        *pending = pending.saturating_add(1);
    }

    fn take_validation_retry(&self, id: u64, generation: u64, pc: u32) -> bool {
        let mut retries = self
            .validation_retries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = (id, generation, pc);
        let Some(pending) = retries.get_mut(&key) else {
            return false;
        };
        *pending -= 1;
        if *pending == 0 {
            retries.remove(&key);
        }
        true
    }

    fn mark_deopt_guard(
        &self,
        id: u64,
        generation: u64,
        guard: u32,
        observed: Option<runtime::ObservedType>,
    ) {
        #[cfg(feature = "test-support")]
        test_deopt_guards()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(runtime::FunctionKey::new(id, generation), guard);
        self.deopt_guards
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry((id, generation))
            .or_default()
            .push_back((guard, observed));
    }

    fn take_deopt_guard(
        &self,
        id: u64,
        generation: u64,
    ) -> Option<(u32, Option<runtime::ObservedType>)> {
        let mut guards = self
            .deopt_guards
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let queue = guards.get_mut(&(id, generation))?;
        let guard = queue.pop_front();
        if queue.is_empty() {
            guards.remove(&(id, generation));
        }
        guard
    }

    fn take_side_path_entries(&self) -> u64 {
        self.side_path_entries.swap(0, Ordering::AcqRel)
    }

    fn mark_native_return(&self, id: u64, generation: u64, observed: runtime::ObservedType) {
        let mut pending = self
            .native_return
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        debug_assert!(pending.is_none(), "native return callback was not consumed");
        *pending = Some(PendingNativeReturn {
            function: runtime::FunctionKey::new(id, generation),
            observed,
        });
    }

    fn take_native_return(&self, id: u64, generation: u64) -> Option<runtime::ObservedType> {
        let mut pending = self
            .native_return
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.as_ref()?.function != runtime::FunctionKey::new(id, generation) {
            return None;
        }
        pending.take().map(|entry| entry.observed)
    }
}

/// Feedback classification of one tagged value, shared by deoptimization
/// and native-return observations.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn observed_value_type(value: rquickjs_core::qjs::JSValue) -> Option<runtime::ObservedType> {
    use rquickjs_core::qjs;
    let tag = unsafe { qjs::JS_VALUE_GET_TAG(value) };
    Some(match tag {
        qjs::JS_TAG_INT => runtime::ObservedType::Int32,
        qjs::JS_TAG_FLOAT64 => runtime::ObservedType::Float64,
        qjs::JS_TAG_BOOL => runtime::ObservedType::Bool,
        qjs::JS_TAG_NULL => runtime::ObservedType::Null,
        qjs::JS_TAG_UNDEFINED => runtime::ObservedType::Undefined,
        qjs::JS_TAG_STRING => runtime::ObservedType::String,
        qjs::JS_TAG_OBJECT => runtime::ObservedType::Object,
        _ => return None,
    })
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn record_osr_validation(metrics: &OsrValidationMetrics, pc: u32, valid: bool) {
    if pc == 0 {
        return;
    }
    if valid {
        metrics.successes.fetch_add(1, Ordering::Relaxed);
    } else {
        metrics.failures.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const fn is_generated_osr_retry(pc: u32, exit_kind: u32, validation_retry: bool) -> bool {
    pc != 0
        && exit_kind == rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_RETRY_INTERPRETER
        && !validation_retry
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn apply_stress_gc_after_validation(
    frame: &mut rquickjs_core::qjs::JSJitExecFrame,
    stress_gc: bool,
    valid: bool,
) {
    if valid && stress_gc {
        frame.flags |= rquickjs_core::qjs::JS_JIT_FRAME_STRESS_GC;
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn retry_exit() -> rquickjs_core::qjs::JSJitExit {
    rquickjs_core::qjs::JSJitExit {
        kind: rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_RETRY_INTERPRETER,
        reserved: 0,
        resume_pc: core::ptr::null(),
        resume_stack_top: core::ptr::null_mut(),
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn validated_native_deopt(
    exit: &rquickjs_core::qjs::JSJitExit,
    bytecode_start: *const u8,
    deopt_sites: &[(ir::OptimizedFrameShape, ir::DeoptMap)],
) -> Option<(u32, ir::OptimizedFrameShape)> {
    let guard = exit.reserved.checked_sub(1)?;
    let resume_pc = (exit.resume_pc as usize)
        .checked_sub(bytecode_start as usize)
        .and_then(|pc| u32::try_from(pc).ok())?;
    let (shape, map) = deopt_sites.iter().find(|(shape, map)| {
        map.guard() == guard && map.resume_pc() == resume_pc && map.validate(*shape).is_ok()
    })?;
    // Production recipes must alias their own already-published owning slot.
    // Non-identity recipes need an owning materializer before they can resume.
    map.validate_identity_materialization(*shape).ok()?;
    Some((guard, *shape))
}

/// Validates Tier 1's in-place deoptimization transaction. Unlike Tier 2,
/// baseline code does not identify a speculative guard: it has already
/// published the complete interpreter frame and returns the historical zero
/// identity. Keep this path tier-specific so a missing Tier 2 map can never be
/// mistaken for a valid baseline exit.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn validated_baseline_deopt(
    exit: &rquickjs_core::qjs::JSJitExit,
    frame: &rquickjs_core::qjs::JSJitExecFrame,
    bytecode_start: *const u8,
    arg_buf: *mut rquickjs_core::qjs::JSValue,
    var_buf: *mut rquickjs_core::qjs::JSValue,
    stack_base: *mut rquickjs_core::qjs::JSValue,
    stack_capacity: *mut rquickjs_core::qjs::JSValue,
) -> bool {
    if exit.reserved != 0
        || exit.resume_pc.is_null()
        || !exit.resume_stack_top.is_null()
        || frame.bytecode_start != bytecode_start
        || frame.pc != exit.resume_pc
        || frame.arg_buf != arg_buf
        || frame.var_buf != var_buf
        || frame.stack_base != stack_base
        || frame.stack_capacity != stack_capacity
    {
        return false;
    }
    let Some(resume_offset) = (exit.resume_pc as usize).checked_sub(bytecode_start as usize) else {
        return false;
    };
    if u32::try_from(resume_offset).is_err() {
        return false;
    }
    let base = stack_base as usize;
    let top = frame.stack_top as usize;
    let capacity = stack_capacity as usize;
    base <= top
        && top <= capacity
        && top
            .saturating_sub(base)
            .is_multiple_of(core::mem::size_of::<rquickjs_core::qjs::JSValue>())
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[derive(Clone, Copy)]
struct TrustedFrameBuffers {
    arg_buf: *mut rquickjs_core::qjs::JSValue,
    var_buf: *mut rquickjs_core::qjs::JSValue,
    stack_base: *mut rquickjs_core::qjs::JSValue,
    stack_capacity: *mut rquickjs_core::qjs::JSValue,
    argument_capacity: usize,
    local_capacity: usize,
    stack_capacity_slots: usize,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
impl TrustedFrameBuffers {
    #[cfg(rquickjs_memory_sanitizer)]
    fn capture(
        frame: &rquickjs_core::qjs::JSJitExecFrame,
        argument_capacity: usize,
        local_capacity: usize,
    ) -> Option<Self> {
        Self::capture_raw(
            frame.arg_buf,
            frame.var_buf,
            frame.stack_base,
            frame.stack_capacity,
            argument_capacity,
            local_capacity,
        )
    }

    fn capture_raw(
        arg_buf: *mut rquickjs_core::qjs::JSValue,
        var_buf: *mut rquickjs_core::qjs::JSValue,
        stack_base: *mut rquickjs_core::qjs::JSValue,
        stack_capacity: *mut rquickjs_core::qjs::JSValue,
        argument_capacity: usize,
        local_capacity: usize,
    ) -> Option<Self> {
        let value_size = core::mem::size_of::<rquickjs_core::qjs::JSValue>();
        let stack = stack_base as usize;
        let capacity = stack_capacity as usize;
        // QuickJS aliases `arg_buf` to the caller's `argv` when no argument
        // copy is needed, and a zero-argument call may pass a null `argv`
        // (the baseline CALL helper does for `argc == 0`). An empty argument
        // or local area therefore may be null; it is never dereferenced.
        if (arg_buf.is_null() && argument_capacity != 0)
            || (var_buf.is_null() && local_capacity != 0)
            || stack_base.is_null()
            || stack_capacity.is_null()
            || stack > capacity
        {
            return None;
        }
        let stack_bytes = capacity.checked_sub(stack)?;
        if stack_bytes % value_size != 0 {
            return None;
        }
        argument_capacity.checked_mul(value_size)?;
        local_capacity.checked_mul(value_size)?;
        Some(Self {
            arg_buf,
            var_buf,
            stack_base,
            stack_capacity,
            argument_capacity,
            local_capacity,
            stack_capacity_slots: stack_bytes / value_size,
        })
    }

    fn validates_optimized_shape(
        self,
        frame: &rquickjs_core::qjs::JSJitExecFrame,
        shape: ir::OptimizedFrameShape,
    ) -> bool {
        if frame.arg_buf != self.arg_buf
            || frame.var_buf != self.var_buf
            || frame.stack_base != self.stack_base
            || frame.stack_capacity != self.stack_capacity
            || usize::from(shape.arguments()) != self.argument_capacity
            || usize::from(shape.locals()) != self.local_capacity
            || usize::from(shape.stack()) > self.stack_capacity_slots
        {
            return false;
        }
        let expected_top = (self.stack_base as usize).checked_add(
            usize::from(shape.stack())
                .saturating_mul(core::mem::size_of::<rquickjs_core::qjs::JSValue>()),
        );
        expected_top == Some(frame.stack_top as usize)
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn precomputed_frame_counts(
    snapshot_counts: Option<(usize, usize)>,
    deopt_sites: &[(ir::OptimizedFrameShape, ir::DeoptMap)],
    osr_counts: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    snapshot_counts
        .or_else(|| {
            let (first_shape, _) = deopt_sites.first()?;
            deopt_sites
                .iter()
                .all(|(shape, _)| {
                    shape.arguments() == first_shape.arguments()
                        && shape.locals() == first_shape.locals()
                })
                .then_some((
                    usize::from(first_shape.arguments()),
                    usize::from(first_shape.locals()),
                ))
        })
        .or(osr_counts)
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
unsafe fn reject_native_deopt(
    validated_ctx: *mut rquickjs_core::qjs::JSContext,
) -> rquickjs_core::qjs::JSJitExit {
    use rquickjs_core::qjs;

    // Native code may already have committed visible effects. RETRY would
    // replay them. Do not inspect rejected frame pointers or release roots:
    // C owns terminal cleanup of the current published frame/inline chain.
    unsafe {
        qjs::JS_ThrowInternalError(
            validated_ctx,
            c"invalid native deoptimization metadata".as_ptr(),
        );
        let exception = qjs::JS_GetException(validated_ctx);
        qjs::JS_SetUncatchableError(validated_ctx, exception);
        qjs::JS_Throw(validated_ctx, exception);
    }
    qjs::JSJitExit {
        // The C INTERRUPT path additionally installs its preallocated
        // uncatchable sentinel if creating the InternalError failed (OOM).
        kind: qjs::JSJitExitKind_JS_JIT_EXIT_INTERRUPT,
        reserved: 0,
        resume_pc: core::ptr::null(),
        resume_stack_top: core::ptr::null_mut(),
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn validate_production_frame(
    frame: &rquickjs_core::qjs::JSJitExecFrame,
    runtime_id: u64,
    key: runtime::FunctionKey,
    pc: u32,
    stack_map_count: u32,
    osr: Option<&runtime::OsrMap>,
) -> bool {
    use bytecode::SlotKind;
    use rquickjs_core::qjs;

    let expected_pc_address = (frame.bytecode_start as usize).checked_add(pc as usize);
    if frame.struct_size != core::mem::size_of::<qjs::JSJitExecFrame>() as u32
        || frame.rt.is_null()
        || frame.ctx.is_null()
        || frame.runtime_api.is_null()
        || frame.runtime_id != runtime_id
        || frame.frame_cookie == 0
        || frame.function_id != key.id
        || frame.generation != key.generation
        || frame.bytecode_start.is_null()
        || frame.pc.is_null()
        || Some(frame.pc as usize) != expected_pc_address
        || frame.entry.stack_map_count != stack_map_count
        || frame.entry.helper_abi_version != qjs::QJSJIT_HELPER_ABI_VERSION
        || frame.stack_base.is_null()
        || frame.stack_top.is_null()
        || frame.stack_capacity.is_null()
        || (frame.stack_base as usize) > (frame.stack_top as usize)
        || (frame.stack_top as usize) > (frame.stack_capacity as usize)
    {
        return false;
    }
    let Some(map) = osr else {
        return pc == 0;
    };
    let value_size = core::mem::size_of::<qjs::JSValue>();
    let expected_top = (frame.stack_base as usize)
        .checked_add(usize::from(map.stack_depth()).saturating_mul(value_size));
    // An empty argument/local area may be null (a zero-argument call can
    // pass a null `argv`); it is never read below.
    if expected_top != Some(frame.stack_top as usize)
        || (frame.arg_buf.is_null() && map.argument_count() != 0)
        || (frame.var_buf.is_null() && map.local_count() != 0)
        || map.live_slots().len()
            != usize::from(map.argument_count())
                + usize::from(map.local_count())
                + usize::from(map.stack_depth())
    {
        return false;
    }
    for (index, kind) in map.live_slots().iter().copied().enumerate() {
        let value = if index < usize::from(map.argument_count()) {
            unsafe { &*frame.arg_buf.add(index) }
        } else if index < usize::from(map.argument_count()) + usize::from(map.local_count()) {
            unsafe { &*frame.var_buf.add(index - usize::from(map.argument_count())) }
        } else {
            unsafe {
                &*frame
                    .stack_base
                    .add(index - usize::from(map.argument_count()) - usize::from(map.local_count()))
            }
        };
        let valid = match kind {
            SlotKind::Tagged => true,
            SlotKind::Int32 => value.tag == i64::from(qjs::JS_TAG_INT),
            SlotKind::CatchOffset => value.tag == i64::from(qjs::JS_TAG_CATCH_OFFSET),
            SlotKind::Float64 => value.tag == i64::from(qjs::JS_TAG_FLOAT64),
            SlotKind::Uninitialized => value.tag == i64::from(qjs::JS_TAG_UNINITIALIZED),
        };
        if !valid {
            return false;
        }
    }
    true
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
unsafe extern "C" fn production_entry_trampoline(
    frame: *mut rquickjs_core::qjs::JSJitExecFrame,
) -> rquickjs_core::qjs::JSJitExit {
    use rquickjs_core::qjs;

    if frame.is_null() {
        return retry_exit();
    }
    let frame = unsafe { &mut *frame };
    if frame.entry.pin.is_null() {
        return retry_exit();
    }
    let pin = unsafe { &*frame.entry.pin.cast::<ProductionEntryPin>() };
    let _keep_execution_pinned = &pin.execution;
    let valid = validate_production_frame(
        frame,
        pin.runtime_id,
        pin.key,
        pin.pc,
        pin.stack_map_count,
        pin.osr.as_ref(),
    );
    record_osr_validation(&pin.validation, pin.pc, valid);
    if !valid {
        pin.validation
            .mark_validation_retry(pin.key.id, pin.key.generation, pin.pc);
        return retry_exit();
    }
    #[cfg(feature = "test-support")]
    apply_stress_gc_after_validation(frame, pin.stress_gc, valid);
    // Capture trusted entry context before native execution can modify the
    // frame. Rejection must not dereference pointers supplied by a bad exit.
    let validated_ctx = frame.ctx;
    let fast_entry = frame.flags & qjs::JS_JIT_FRAME_FAST_ENTRY != 0;
    let validated_bytecode_start = frame.bytecode_start;
    let validated_arg_buf = frame.arg_buf;
    let validated_var_buf = frame.var_buf;
    let validated_stack_base = frame.stack_base;
    let validated_stack_capacity = frame.stack_capacity;
    #[cfg(rquickjs_memory_sanitizer)]
    let validated_buffers = pin
        .frame_counts
        .and_then(|(arguments, locals)| TrustedFrameBuffers::capture(frame, arguments, locals));
    type NativeEntry = unsafe extern "C" fn(*mut qjs::JSJitExecFrame) -> qjs::JSJitExit;
    let native = unsafe { core::mem::transmute::<*const u8, NativeEntry>(pin.native) };
    let mut exit = unsafe { native(frame as *const _ as *mut _) };
    #[cfg(rquickjs_memory_sanitizer)]
    unsafe {
        unsafe extern "C" {
            fn __msan_unpoison(address: *const core::ffi::c_void, size: usize);
        }
        __msan_unpoison(
            core::ptr::addr_of!(exit).cast(),
            core::mem::size_of::<qjs::JSJitExit>(),
        );
        __msan_unpoison(
            (frame as *mut qjs::JSJitExecFrame).cast(),
            core::mem::size_of::<qjs::JSJitExecFrame>(),
        );
        if let Some(buffers) = validated_buffers {
            let value_size = core::mem::size_of::<qjs::JSValue>();
            let argument_bytes = buffers.argument_capacity.saturating_mul(value_size);
            if argument_bytes <= 64 * 1024 * 1024 {
                __msan_unpoison(buffers.arg_buf.cast(), argument_bytes);
            }
            let local_bytes = buffers.local_capacity.saturating_mul(value_size);
            if local_bytes <= 64 * 1024 * 1024 {
                __msan_unpoison(buffers.var_buf.cast(), local_bytes);
            }
            let stack_bytes = buffers.stack_capacity_slots.saturating_mul(value_size);
            if stack_bytes <= 64 * 1024 * 1024 {
                __msan_unpoison(buffers.stack_base.cast(), stack_bytes);
            }
        }
    }
    if frame.flags & qjs::JS_JIT_FRAME_SIDE_PATH_HIT != 0 {
        frame.flags &= !qjs::JS_JIT_FRAME_SIDE_PATH_HIT;
        pin.validation
            .side_path_entries
            .fetch_add(1, Ordering::Release);
    }
    // A granted fast DONE exit reports no native_exit to consume this
    // handoff; its steady return type is resampled on the next full call.
    if exit.kind == qjs::JSJitExitKind_JS_JIT_EXIT_DONE && !fast_entry {
        if let Some(observed) = observed_value_type(frame.result) {
            pin.validation
                .mark_native_return(pin.key.id, pin.key.generation, observed);
        }
    }
    if exit.kind != qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT {
        return exit;
    }
    if pin.execution.artifact().key().tier == runtime::Tier::Baseline {
        if validated_baseline_deopt(
            &exit,
            frame,
            validated_bytecode_start,
            validated_arg_buf,
            validated_var_buf,
            validated_stack_base,
            validated_stack_capacity,
        ) {
            return exit;
        }
        return unsafe { reject_native_deopt(validated_ctx) };
    }
    // The pinned artifact outlives this entry, so its deopt table is read in
    // place instead of being copied on every native entry.
    let deopt_sites = pin
        .execution
        .artifact()
        .optimized_metadata()
        .map_or(&[][..], |metadata| metadata.deopt_sites());
    let Some((guard, shape)) = validated_native_deopt(&exit, validated_bytecode_start, deopt_sites)
    else {
        return unsafe { reject_native_deopt(validated_ctx) };
    };
    // Materialization is cold. Build the trusted buffer descriptor only after
    // a DEOPT, from the pointers captured before native code could mutate the
    // frame. The normal DONE path now pays only scalar pointer snapshots.
    let validated_buffers = pin.frame_counts.and_then(|(arguments, locals)| {
        TrustedFrameBuffers::capture_raw(
            validated_arg_buf,
            validated_var_buf,
            validated_stack_base,
            validated_stack_capacity,
            arguments,
            locals,
        )
    });
    let Some(validated_buffers) =
        validated_buffers.filter(|buffers| buffers.validates_optimized_shape(frame, shape))
    else {
        return unsafe { reject_native_deopt(validated_ctx) };
    };
    pin.validation
        .deopt_materializations
        .fetch_add(1, Ordering::Relaxed);
    pin.validation.mark_deopt_guard(
        pin.key.id,
        pin.key.generation,
        guard,
        observed_deopt_type(validated_buffers, shape),
    );
    /* The one-based identity is internal to the pinned backend artifact. C's
     * stable ABI keeps this field reserved and receives only validated zero. */
    exit.reserved = 0;
    exit
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn observed_deopt_type(
    buffers: TrustedFrameBuffers,
    shape: ir::OptimizedFrameShape,
) -> Option<runtime::ObservedType> {
    use rquickjs_core::qjs;
    // Empty areas may be null (see `TrustedFrameBuffers::capture_raw`), and a
    // slice may not be built from a null pointer even when it is empty.
    let area = |pointer: *mut qjs::JSValue, count: u16| -> &[qjs::JSValue] {
        if count == 0 {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(pointer, usize::from(count)) }
        }
    };
    let values = area(buffers.arg_buf, shape.arguments())
        .iter()
        .chain(area(buffers.var_buf, shape.locals()));
    values
        .filter_map(|value| {
            let tag = unsafe { qjs::JS_VALUE_GET_TAG(*value) };
            Some(match tag {
                qjs::JS_TAG_INT => runtime::ObservedType::Int32,
                qjs::JS_TAG_FLOAT64 => runtime::ObservedType::Float64,
                qjs::JS_TAG_BOOL => runtime::ObservedType::Bool,
                qjs::JS_TAG_NULL => runtime::ObservedType::Null,
                qjs::JS_TAG_UNDEFINED => runtime::ObservedType::Undefined,
                qjs::JS_TAG_STRING => runtime::ObservedType::String,
                qjs::JS_TAG_OBJECT => runtime::ObservedType::Object,
                _ => return None,
            })
        })
        .find(|observed| *observed != runtime::ObservedType::Int32)
}

#[cfg(all(test, feature = "compiler", not(target_family = "wasm")))]
mod production_osr_validation_tests {
    use super::*;
    use rquickjs_core::qjs;

    #[test]
    fn tier2_profitability_trial_is_classified_exactly_once() {
        let mut profile = ProductionProfile::default();
        for _ in 0..8 {
            profile.record_baseline(100, true);
        }
        for _ in 0..7 {
            profile.record_optimized(80);
        }
        assert_eq!(classify_tier2_trial(&mut profile), None);
        profile.record_optimized(80);
        assert_eq!(classify_tier2_trial(&mut profile), Some(false));
        assert!(profile.tier2_trial_decided);
        // Later samples must not revisit the cross-multiplied decision.
        for _ in 0..8 {
            profile.record_optimized(u64::MAX);
        }
        assert_eq!(classify_tier2_trial(&mut profile), None);

        let mut slow = ProductionProfile::default();
        for _ in 0..8 {
            slow.record_baseline(100, true);
        }
        for _ in 0..8 {
            slow.record_optimized(125);
        }
        assert_eq!(classify_tier2_trial(&mut slow), Some(true));
        assert!(slow.tier2_trial_decided);
    }

    #[test]
    fn tier2_profitability_trial_ignores_one_scheduling_outlier() {
        // Sub-microsecond callees are timed through instrumented callbacks
        // (sanitizers, debug hosts); one preempted sample in the window must
        // not condemn a Tier-2 version whose steady state is faster.
        let mut profile = ProductionProfile::default();
        for _ in 0..8 {
            profile.record_baseline(600, true);
        }
        for _ in 0..7 {
            profile.record_optimized(400);
        }
        profile.record_optimized(5_000);
        assert_eq!(profile.optimized_min_ns, 400);
        assert_eq!(classify_tier2_trial(&mut profile), Some(false));
        assert!(profile.tier2_trial_decided);
    }

    #[test]
    fn entry_validation_counts_are_precomputed_once_from_immutable_metadata() {
        let shape = ir::OptimizedFrameShape::new(3, 4, 1);
        let other_stack_depth = ir::OptimizedFrameShape::new(3, 4, 7);
        assert_eq!(
            precomputed_frame_counts(
                None,
                &[
                    (shape, dummy_deopt_map(shape)),
                    (other_stack_depth, dummy_deopt_map(other_stack_depth))
                ],
                None
            ),
            Some((3, 4)),
            "stack depth may vary while the argument/local buffers stay fixed"
        );
        let conflicting = ir::OptimizedFrameShape::new(3, 5, 1);
        assert_eq!(
            precomputed_frame_counts(
                None,
                &[
                    (shape, dummy_deopt_map(shape)),
                    (conflicting, dummy_deopt_map(conflicting))
                ],
                None
            ),
            None,
            "conflicting immutable metadata must fail closed before native entry"
        );
        assert_eq!(
            precomputed_frame_counts(
                Some((8, 9)),
                &[(conflicting, dummy_deopt_map(conflicting))],
                Some((1, 2))
            ),
            Some((8, 9)),
            "the retained bytecode snapshot is the authoritative entry shape"
        );
    }

    fn dummy_deopt_map(shape: ir::OptimizedFrameShape) -> ir::DeoptMap {
        let slots = (0..shape.arguments())
            .map(|index| {
                ir::Materialization::argument(index, ir::MaterializedValue::TaggedSlot(index))
            })
            .chain((0..shape.locals()).map(|index| {
                ir::Materialization::local(
                    index,
                    ir::MaterializedValue::TaggedSlot(shape.arguments() + index),
                )
            }))
            .chain((0..shape.stack()).map(|index| {
                ir::Materialization::stack(
                    index,
                    ir::MaterializedValue::TaggedSlot(shape.arguments() + shape.locals() + index),
                )
            }))
            .collect();
        ir::DeoptMap::new(0, 0, ir::DeoptPhase::BeforeEffect(0), slots)
    }

    unsafe extern "C" fn native_effect_then_deopt(
        frame: *mut qjs::JSJitExecFrame,
    ) -> qjs::JSJitExit {
        let frame = unsafe { &mut *frame };
        let case = unsafe { (*frame.arg_buf).u.int32 };
        unsafe { (*frame.var_buf).u.int32 += 1 };
        frame.pc = unsafe { frame.bytecode_start.add(2) };
        match case {
            6 => frame.arg_buf = core::ptr::NonNull::dangling().as_ptr(),
            7 => frame.var_buf = core::ptr::NonNull::dangling().as_ptr(),
            8 => frame.stack_base = core::ptr::NonNull::dangling().as_ptr(),
            9 => frame.stack_capacity = core::ptr::NonNull::dangling().as_ptr(),
            10 => frame.stack_top = core::ptr::NonNull::dangling().as_ptr(),
            _ => {}
        }
        qjs::JSJitExit {
            kind: qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
            reserved: match case {
                0 => 0,  // Missing one-based guard identity.
                2 => 99, // Unknown guard identity.
                _ => 1,
            },
            resume_pc: if case == 1 {
                core::ptr::null()
            } else {
                frame.pc
            },
            resume_stack_top: frame.stack_top,
        }
    }

    #[test]
    fn invalid_post_native_deopts_are_terminal_without_replaying_effects_or_touching_roots() {
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();
        context.with(|ctx| {
            for case in 0..11 {
                let shape = ir::OptimizedFrameShape::new(1, 1, 1);
                let slots = if case == 3 {
                    vec![] // Known guard, malformed complete-frame recipe.
                } else {
                    vec![
                        ir::Materialization::argument(
                            0,
                            if case == 4 {
                                ir::MaterializedValue::Int32(7) // Valid, but not identity.
                            } else {
                                ir::MaterializedValue::TaggedSlot(0)
                            },
                        ),
                        ir::Materialization::local(0, ir::MaterializedValue::TaggedSlot(1)),
                        ir::Materialization::stack(0, ir::MaterializedValue::TaggedSlot(2)),
                    ]
                };
                let artifact_key = code_cache::ArtifactKey {
                    runtime_id: 11,
                    function_id: 7,
                    generation: 3,
                    tier: runtime::Tier::Optimizing,
                    target_isa: 0,
                    cpu_features: 0,
                    abi_fingerprint: 0,
                    source_revision: 0,
                    opcode_fingerprint: 0,
                    config_fingerprint: 0,
                    specialization_fingerprint: 0,
                };
                let metadata = code_cache::OptimizedArtifactMetadata::new(
                    0,
                    vec![(
                        shape,
                        ir::DeoptMap::new(0, 2, ir::DeoptPhase::AfterEffect(1), slots),
                    )],
                    0,
                    0,
                    0,
                );
                let mut cache = code_cache::CodeCache::new(1 << 20);
                cache
                    .insert(code_cache::CompiledArtifact::empty(
                        artifact_key.with_tier(runtime::Tier::Baseline),
                    ))
                    .unwrap();
                cache
                    .insert(
                        code_cache::CompiledArtifact::empty(artifact_key)
                            .with_optimized_metadata(metadata),
                    )
                    .unwrap();
                let pin = ProductionEntryPin {
                    execution: cache.pin(artifact_key).unwrap(),
                    native: native_effect_then_deopt as *const u8,
                    runtime_id: 11,
                    key: runtime::FunctionKey::new(7, 3),
                    pc: 0,
                    stack_map_count: 0,
                    osr: None,
                    frame_counts: Some((1, 1)),
                    validation: Arc::new(OsrValidationMetrics::default()),
                    #[cfg(feature = "test-support")]
                    stress_gc: false,
                };
                let mut bytecode = [0_u8; 8];
                let mut arguments = [qjs::JS_MKVAL(qjs::JS_TAG_INT, case)];
                let mut locals = [qjs::JS_MKVAL(qjs::JS_TAG_INT, 0)];
                let root: rquickjs::Value = ctx.eval("({live: 'published root'})").unwrap();
                let mut stack = [root.as_raw()];
                let stack_before = stack[0];
                let mut frame: qjs::JSJitExecFrame = unsafe { core::mem::zeroed() };
                frame.struct_size = core::mem::size_of::<qjs::JSJitExecFrame>() as u32;
                frame.ctx = ctx.as_raw().as_ptr();
                frame.rt = unsafe { qjs::JS_GetRuntime(frame.ctx) };
                frame.runtime_id = 11;
                frame.function_id = 7;
                frame.generation = 3;
                frame.frame_cookie = 9;
                frame.runtime_api = core::ptr::NonNull::dangling().as_ptr();
                frame.arg_buf = arguments.as_mut_ptr();
                frame.var_buf = locals.as_mut_ptr();
                frame.stack_base = stack.as_mut_ptr();
                frame.stack_top = unsafe { stack.as_mut_ptr().add(1) };
                frame.stack_capacity = frame.stack_top;
                frame.bytecode_start = bytecode.as_mut_ptr();
                frame.pc = bytecode.as_mut_ptr();
                frame.result = qjs::JS_UNDEFINED;
                frame.entry.pin = (&pin as *const ProductionEntryPin).cast_mut().cast();
                frame.entry.helper_abi_version = qjs::QJSJIT_HELPER_ABI_VERSION;
                // Rejection before entry may still retry: native work has not
                // run, no exception is installed, and every root is intact.
                frame.frame_cookie = 0;
                let rejected_before = bytes(&frame);
                let rejected = unsafe { production_entry_trampoline(&mut frame) };
                assert_eq!(
                    rejected.kind,
                    qjs::JSJitExitKind_JS_JIT_EXIT_RETRY_INTERPRETER
                );
                assert_eq!(unsafe { locals[0].u.int32 }, 0);
                assert_eq!(bytes(&frame), rejected_before);
                assert!(!unsafe { qjs::JS_HasException(frame.ctx) });
                frame.frame_cookie = 9;
                let mut expected_frame = frame;
                expected_frame.pc = unsafe { bytecode.as_ptr().add(2) };
                match case {
                    6 => expected_frame.arg_buf = core::ptr::NonNull::dangling().as_ptr(),
                    7 => expected_frame.var_buf = core::ptr::NonNull::dangling().as_ptr(),
                    8 => expected_frame.stack_base = core::ptr::NonNull::dangling().as_ptr(),
                    9 => expected_frame.stack_capacity = core::ptr::NonNull::dangling().as_ptr(),
                    10 => expected_frame.stack_top = core::ptr::NonNull::dangling().as_ptr(),
                    _ => {}
                }

                let exit = unsafe { production_entry_trampoline(&mut frame) };
                assert_eq!(unsafe { locals[0].u.int32 }, 1, "native effect runs once");
                assert_eq!(
                    bytes(&frame),
                    bytes(&expected_frame),
                    "case {case}: preserve C cleanup roots"
                );
                assert_eq!(stack[0].tag, stack_before.tag);
                assert_eq!(unsafe { stack[0].u.ptr }, unsafe { stack_before.u.ptr });
                if case == 5 {
                    assert_eq!(exit.kind, qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT);
                    assert_eq!(exit.reserved, 0);
                    assert_eq!(
                        pin.validation
                            .deopt_materializations
                            .load(Ordering::Relaxed),
                        1
                    );
                } else {
                    assert_eq!(
                        exit.kind,
                        qjs::JSJitExitKind_JS_JIT_EXIT_INTERRUPT,
                        "case {case}: never replay native effects"
                    );
                    assert_eq!(exit.reserved, 0);
                    assert!(exit.resume_pc.is_null());
                    assert!(exit.resume_stack_top.is_null());
                    let exception = unsafe { qjs::JS_GetException(frame.ctx) };
                    let uncatchable = unsafe { qjs::JS_IsUncatchableError(exception) };
                    unsafe { qjs::JS_FreeValue(frame.ctx, exception) };
                    assert!(uncatchable, "case {case}: terminal invariant error");
                    assert_eq!(
                        pin.validation
                            .deopt_materializations
                            .load(Ordering::Relaxed),
                        0
                    );
                }
            }
        });
    }

    #[test]
    fn baseline_deopt_accepts_only_the_exact_published_frame_transaction() {
        let mut bytecode = [0_u8; 8];
        let mut arguments = [qjs::JS_UNDEFINED];
        let mut locals = [qjs::JS_UNDEFINED];
        let mut stack = [qjs::JS_UNDEFINED; 2];
        let mut frame: qjs::JSJitExecFrame = unsafe { core::mem::zeroed() };
        frame.arg_buf = arguments.as_mut_ptr();
        frame.var_buf = locals.as_mut_ptr();
        frame.stack_base = stack.as_mut_ptr();
        frame.stack_top = unsafe { stack.as_mut_ptr().add(1) };
        frame.stack_capacity = unsafe { stack.as_mut_ptr().add(2) };
        frame.bytecode_start = bytecode.as_mut_ptr();
        frame.pc = unsafe { bytecode.as_mut_ptr().add(3) };
        let bytecode_start = bytecode.as_ptr();
        let arg_buf = arguments.as_mut_ptr();
        let var_buf = locals.as_mut_ptr();
        let stack_base = stack.as_mut_ptr();
        let stack_capacity = unsafe { stack_base.add(2) };
        let exit = qjs::JSJitExit {
            kind: qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
            reserved: 0,
            resume_pc: frame.pc,
            resume_stack_top: core::ptr::null_mut(),
        };
        let valid = |candidate: &qjs::JSJitExit, published: &qjs::JSJitExecFrame| {
            validated_baseline_deopt(
                candidate,
                published,
                bytecode_start,
                arg_buf,
                var_buf,
                stack_base,
                stack_capacity,
            )
        };
        assert!(valid(&exit, &frame));

        let mut candidate = exit;
        candidate.reserved = 1;
        assert!(!valid(&candidate, &frame));
        candidate = exit;
        candidate.resume_pc = core::ptr::null();
        assert!(!valid(&candidate, &frame));
        candidate = exit;
        candidate.resume_stack_top = frame.stack_top;
        assert!(!valid(&candidate, &frame));

        let mut corrupted = frame;
        corrupted.pc = bytecode_start.cast_mut();
        assert!(!valid(&exit, &corrupted));
        corrupted = frame;
        corrupted.stack_top = unsafe { stack_capacity.cast::<u8>().add(1).cast() };
        assert!(!valid(&exit, &corrupted));
        corrupted = frame;
        corrupted.stack_capacity = frame.stack_top;
        assert!(!valid(&exit, &corrupted));
    }

    #[test]
    fn native_return_handoff_preserves_identity_type_and_recursive_exit_order() {
        let metrics = OsrValidationMetrics::default();
        // An inner trampoline completes and its C exit callback consumes the
        // observation before the outer native call can finish.
        for (id, generation, observed) in [
            (7, 3, runtime::ObservedType::Int32),
            (7, 3, runtime::ObservedType::Float64),
            (8, 4, runtime::ObservedType::String),
            (7, 5, runtime::ObservedType::Object),
            (7, 5, runtime::ObservedType::Bool),
        ] {
            metrics.mark_native_return(id, generation, observed);
            assert_eq!(metrics.take_native_return(id + 1, generation), None);
            assert_eq!(metrics.take_native_return(id, generation + 1), None);
            assert_eq!(metrics.take_native_return(id, generation), Some(observed));
            assert_eq!(metrics.take_native_return(id, generation), None);
        }
    }

    fn bytes(frame: &qjs::JSJitExecFrame) -> Vec<u8> {
        unsafe {
            core::slice::from_raw_parts(
                (frame as *const qjs::JSJitExecFrame).cast::<u8>(),
                core::mem::size_of::<qjs::JSJitExecFrame>(),
            )
            .to_vec()
        }
    }

    #[test]
    fn every_invalid_osr_field_retries_without_mutating_the_frame() {
        let mut bytecode = [0_u8; 8];
        let mut stack = [unsafe { core::mem::zeroed::<qjs::JSValue>() }; 2];
        stack[0].tag = i64::from(qjs::JS_TAG_INT);
        let dangling_value = core::ptr::NonNull::<qjs::JSValue>::dangling().as_ptr();
        let key = runtime::FunctionKey::new(7, 3);
        let map = runtime::OsrMap::new(
            runtime::OsrKey::new(key, 4),
            4,
            1,
            vec![bytecode::SlotKind::Int32],
        );
        let mut frame: qjs::JSJitExecFrame = unsafe { core::mem::zeroed() };
        frame.struct_size = core::mem::size_of::<qjs::JSJitExecFrame>() as u32;
        frame.rt = core::ptr::NonNull::dangling().as_ptr();
        frame.ctx = core::ptr::NonNull::dangling().as_ptr();
        frame.function_id = key.id;
        frame.generation = key.generation;
        frame.arg_buf = dangling_value;
        frame.var_buf = dangling_value;
        frame.stack_base = stack.as_mut_ptr();
        frame.stack_top = unsafe { stack.as_mut_ptr().add(1) };
        frame.stack_capacity = unsafe { stack.as_mut_ptr().add(2) };
        frame.bytecode_start = bytecode.as_mut_ptr();
        frame.pc = unsafe { bytecode.as_mut_ptr().add(4) };
        frame.runtime_api = core::ptr::NonNull::dangling().as_ptr();
        frame.runtime_id = 11;
        frame.frame_cookie = 9;
        frame.entry.stack_map_count = 5;
        frame.entry.helper_abi_version = qjs::QJSJIT_HELPER_ABI_VERSION;
        assert!(validate_production_frame(&frame, 11, key, 4, 5, Some(&map)));

        let mut invalid = Vec::new();
        let mut value = frame;
        value.runtime_id = 12;
        invalid.push(value);
        let mut value = frame;
        value.function_id = 8;
        invalid.push(value);
        let mut value = frame;
        value.generation = 4;
        invalid.push(value);
        let mut value = frame;
        value.pc = bytecode.as_mut_ptr();
        invalid.push(value);
        let mut value = frame;
        value.frame_cookie = 0;
        invalid.push(value);
        let mut value = frame;
        value.entry.stack_map_count = 4;
        invalid.push(value);
        let mut value = frame;
        value.entry.helper_abi_version = 0;
        invalid.push(value);
        let mut value = frame;
        value.stack_top = value.stack_base;
        invalid.push(value);

        for candidate in &invalid {
            let before = bytes(candidate);
            assert!(!validate_production_frame(
                candidate,
                11,
                key,
                4,
                5,
                Some(&map)
            ));
            assert_eq!(bytes(candidate), before);
        }
        let before = bytes(&frame);
        stack[0].tag = i64::from(qjs::JS_TAG_UNDEFINED);
        let tag_before = stack[0].tag;
        assert!(!validate_production_frame(
            &frame,
            11,
            key,
            4,
            5,
            Some(&map)
        ));
        assert_eq!(
            bytes(&frame),
            before,
            "slot rejection does not mutate frame pointers"
        );
        assert_eq!(stack[0].tag, tag_before, "slot owner/refcount is untouched");
    }

    #[test]
    fn osr_success_is_counted_only_after_frame_validation() {
        let metrics = OsrValidationMetrics::default();
        record_osr_validation(&metrics, 9, false);
        assert_eq!(metrics.successes.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.failures.load(Ordering::Relaxed), 1);
        assert!(is_generated_osr_retry(
            9,
            qjs::JSJitExitKind_JS_JIT_EXIT_RETRY_INTERPRETER,
            false,
        ));
        assert!(!is_generated_osr_retry(
            0,
            qjs::JSJitExitKind_JS_JIT_EXIT_RETRY_INTERPRETER,
            false,
        ));
        assert!(!is_generated_osr_retry(
            9,
            qjs::JSJitExitKind_JS_JIT_EXIT_DONE,
            false,
        ));
        assert!(!is_generated_osr_retry(
            9,
            qjs::JSJitExitKind_JS_JIT_EXIT_RETRY_INTERPRETER,
            true,
        ));
        record_osr_validation(&metrics, 9, true);
        assert_eq!(metrics.successes.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.failures.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn invalid_stress_frame_is_byte_identical_and_does_not_arm_stress_gc() {
        let mut frame: qjs::JSJitExecFrame = unsafe { core::mem::zeroed() };
        let before = bytes(&frame);
        apply_stress_gc_after_validation(&mut frame, true, false);
        assert_eq!(bytes(&frame), before);
    }

    #[test]
    fn validation_retry_is_not_attributed_to_generated_native_code() {
        let metrics = OsrValidationMetrics::default();
        metrics.mark_validation_retry(7, 3, 9);
        assert!(metrics.take_validation_retry(7, 3, 9));
        assert!(!metrics.take_validation_retry(7, 3, 9));
        assert!(!metrics.take_validation_retry(7, 3, 10));
    }
}

#[cfg(all(
    feature = "compiler",
    any(
        all(
            target_os = "macos",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "windows",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "linux",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )
))]
impl ProductionBackend {
    fn new(
        runtime_id: u64,
        info: &abi::AbiInfo,
        config: JitConfig,
        metrics: Arc<Mutex<JitMetrics>>,
    ) -> Result<Self, JitError> {
        let environment = Arc::new(Mutex::new(None));
        let compiler_measurements = Arc::new(CompilerMeasurements::default());
        let compiler = Arc::new(MeasuredCompiler {
            inner: DeferredTieredCompiler::default(),
            measurements: Arc::clone(&compiler_measurements),
        });
        let workers = runtime::BackgroundCompiler::new_with_resource_limits(
            compiler,
            config.workers(),
            config.max_queue_len(),
            std::time::Duration::from_millis(config.compile_timeout_ms()),
            config.max_snapshot_bytes(),
            config.max_ir_bytes(),
        )
        .map_err(|_| JitError::InvalidConfig("workers"))?;
        let mut coordinator = runtime::Coordinator::with_environment_and_metadata_limit(
            config.max_queue_len(),
            config.max_queue_len(),
            config.max_compile_attempts(),
            config.max_code_bytes(),
            config.max_metadata_bytes(),
            runtime::ArtifactEnvironment {
                runtime_id,
                ..runtime::ArtifactEnvironment::default()
            },
        );
        coordinator.set_native_enabled(NATIVE_EXECUTION_SUPPORTED);
        let feedback_capacity = config.max_queue_len().max(32);
        Ok(Self {
            runtime_id,
            abi_info: *info,
            environment,
            coordinator,
            config,
            workers,
            requested: std::collections::HashSet::new(),
            hotness: rustc_hash::FxHashMap::default(),
            optimizing_requested: std::collections::HashSet::new(),
            optimizing_hotness: rustc_hash::FxHashMap::default(),
            optimizing_snapshots: std::collections::HashMap::new(),
            baseline_property_refreshed: std::collections::HashSet::new(),
            optimized_vocabulary: std::collections::HashMap::new(),
            tier2_sources: std::collections::HashMap::new(),
            feedback: runtime::FeedbackTable::new(feedback_capacity, 3),
            call_feedback_types: Vec::new(),
            shape_feedback: runtime::ShapeFeedbackTable::new(runtime::POLYMORPHIC_PROPERTY_LIMIT),
            metrics,
            cold_metrics_dirty: true,
            native_entries: 0,
            native_acquisitions: 0,
            native_exits: 0,
            native_fallbacks: 0,
            native_retries: 0,
            osr_entries: 0,
            osr_not_ready: 0,
            osr_map_misses: 0,
            osr_validation_failures: 0,
            osr_attempts: 0,
            osr_generated_retries: 0,
            osr_validation: Arc::new(OsrValidationMetrics::default()),
            clock: 0,
            hot_ticks_since_maintenance: 0,
            generic_call_rejections: 0,
            last_scan_feedback_version: u64::MAX,
            last_scan_installed: u64::MAX,
            last_refresh_scan: None,
            tier2_deferred: rustc_hash::FxHashMap::default(),
            direct_refresh_probes: std::collections::HashMap::new(),
            queue_reasons: std::collections::HashMap::new(),
            prequeue_backoff: std::collections::HashMap::new(),
            hot_call_queues: 0,
            hot_loop_queues: 0,
            adaptive_neutral_queues: 0,
            adaptive_inputs_recorded: 0,
            snapshot_requests: 0,
            stable_path_compile_requests: 0,
            execution_starts: Vec::new(),
            entry_tiers: rustc_hash::FxHashMap::default(),
            entry_cache_epoch: 1,
            fast_entry: Arc::new(FastEntryShared::new(1)),
            execution_profiles: rustc_hash::FxHashMap::default(),
            profitability_evaluations: 0,
            profitability_approved: 0,
            profitability_rejected: 0,
            profitability_backoff: std::collections::HashMap::new(),
            next_profitability_retry: u64::MAX,
            profitability_blacklisted: rustc_hash::FxHashSet::default(),
            tier2_untranslatable: rustc_hash::FxHashSet::default(),
            native_recursive: rustc_hash::FxHashSet::default(),
            native_call_plans: rustc_hash::FxHashMap::default(),
            feedback_disabled: rustc_hash::FxHashSet::default(),
            benefit_recordings: 0,
            measured_benefit_ns: 0,
            compiler_measurements,
            install_ns: 0,
            peak_compiler_bytes: 0,
            #[cfg(feature = "test-support")]
            test_last_acquired_key: Arc::new(Mutex::new(None)),
        })
    }

    fn ensure_artifact_environment(&mut self) {
        if self
            .environment
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
        {
            return;
        }
        let identity_compiler = compiler::baseline::BaselineCompiler::host();
        let environment = artifact_environment(
            self.runtime_id,
            &self.abi_info,
            &self.config,
            &identity_compiler,
        );
        self.coordinator.initialize_environment(environment);
        *self
            .environment
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(environment);
    }

    /// Runs on the per-call native path. Skips the full maintenance pass
    /// while nothing that feeds it has changed, so a steady-state native call
    /// pays only a clock tick instead of completion draining, candidate scans
    /// and feedback snapshots on every entry.
    fn maintenance_if_due(&mut self, publish_metrics: bool) {
        self.hot_ticks_since_maintenance = self.hot_ticks_since_maintenance.saturating_add(1);
        let due = self.hot_ticks_since_maintenance >= HOT_MAINTENANCE_INTERVAL
            || self.coordinator.has_pending_work()
            || self.feedback.version() != self.last_scan_feedback_version
            || self.coordinator.installed_count() != self.last_scan_installed
            || self.clock >= self.next_profitability_retry;
        if due {
            self.maintenance();
            // Only future retries can make the next maintenance due; one the
            // scan did not reach waits for ordinary maintenance.
            let clock = self.clock;
            self.next_profitability_retry = self
                .profitability_backoff
                .values()
                .map(|&(_, retry_at)| retry_at)
                .filter(|&retry_at| retry_at > clock)
                .min()
                .unwrap_or(u64::MAX);
        } else {
            self.clock = self.clock.saturating_add(1);
            self.coordinator.advance_clock(self.clock);
            if publish_metrics {
                self.publish_metrics();
            }
        }
    }

    /// The tier of a cached entry for which per-call callbacks can no longer
    /// change any decision: either its optimized artifact is installed and
    /// the bounded profitability trial is over (or cannot run without
    /// baseline samples), or its baseline artifact is installed and the
    /// optimizing tier is terminally blacklisted, so neither hot counts nor
    /// baseline timing feed a pending promotion. Hot counts, call feedback
    /// and timing are then only sampled on the full call that renews each
    /// fast-entry grant.
    fn fast_entry_steady(&self, key: runtime::FunctionKey) -> Option<runtime::Tier> {
        if self.feedback_disabled.contains(&key) {
            return None;
        }
        let tier = *self.entry_tiers.get(&key)?;
        let optimizing = self.coordinator.tier_state(key, runtime::Tier::Optimizing);
        let steady = match tier {
            runtime::Tier::Optimizing => {
                matches!(optimizing, runtime::CompileState::Installed(_))
                    && self.execution_profiles.get(&key).is_some_and(|profile| {
                        profile.tier2_trial_decided || profile.baseline_executions == 0
                    })
            }
            runtime::Tier::Baseline => {
                // Baseline-only refresh probing consumes every hot event.
                self.config.tier_policy() != JitTierPolicy::BaselineOnly
                    && optimizing == runtime::CompileState::Blacklisted
                    && matches!(
                        self.coordinator.tier_state(key, runtime::Tier::Baseline),
                        runtime::CompileState::Installed(_)
                    )
                    && !self.optimizing_requested.contains(&key)
                    && !self.optimizing_snapshots.contains_key(&key)
            }
        };
        steady.then_some(tier)
    }

    fn invalidate_entry_cache(&mut self) {
        // Epoch zero permanently disables caching after exhaustion: never let
        // an old handle become admissible again through wraparound.
        if self.entry_cache_epoch != 0 {
            self.entry_cache_epoch = self.entry_cache_epoch.checked_add(1).unwrap_or(0);
        }
        // Granted QuickJS handles compare this copy instead of querying
        // `entry_cache_epoch`, so publish every change before returning.
        self.fast_entry
            .epoch
            .store(self.entry_cache_epoch, Ordering::Relaxed);
    }

    /// Settles a generation that Tier 2 can never translate and whose Tier 1
    /// code exists only because of the opcodes in
    /// `SETTLED_GENERIC_TIER1_OPCODES` (see `settles_untranslatable_candidate`).
    /// Such a function ran in the interpreter at 82d3808, and its baseline is
    /// dominated by exact slow-path helper calls: the optimizing scan would
    /// otherwise revisit it at every maintenance, rebuilding feedback
    /// snapshots whenever a property/call/numeric gate defers the trial, and
    /// the new `methods-dynamic` kernel ran 5.4x slower that way. After a short
    /// baseline warmup it returns to the interpreter with its probes off, the
    /// state it had at 82d3808. The call-only rule runs first so that its
    /// metric keeps its meaning.
    fn settle_untranslatable_tier2_candidate(&mut self, key: runtime::FunctionKey) {
        let Some(snapshot) = self.optimizing_snapshots.get(&key) else {
            return;
        };
        if self.profitability_blacklisted.contains(&key) {
            self.retire_untranslatable_to_interpreter(key);
            return;
        }
        // Let the installed baseline serve a short warmup (and any OSR it
        // enabled) first. The deferral is a map lookup; no feedback snapshot
        // is built.
        if self
            .execution_profiles
            .get(&key)
            .is_none_or(|profile| profile.baseline_executions < UNTRANSLATABLE_SETTLE_EXECUTIONS)
        {
            return;
        }
        let has_loop = snapshot.control_flow_graph().blocks().iter().any(|block| {
            snapshot
                .control_flow_graph()
                .is_loop_header(block.start_pc())
        });
        // Both outcomes retire to the interpreter; only the call-only
        // demotion metric differs.
        if !has_loop {
            let observed = self.feedback.snapshot(self.clock.max(1));
            let generic_call = snapshot.instructions().iter().any(|instruction| {
                matches!(
                    instruction.opcode().name(),
                    "call"
                        | "call0"
                        | "call1"
                        | "call2"
                        | "call3"
                        | "call_method"
                        | "tail_call"
                        | "tail_call_method"
                        | "call_constructor"
                ) && !observed
                    .call_specialization_at(key, instruction.pc())
                    .is_some_and(|call| {
                        call.callee() != key && self.coordinator.direct_call_ready(&call)
                    })
            });
            if generic_call {
                self.cold_metrics_dirty = true;
                self.generic_call_rejections = self.generic_call_rejections.saturating_add(1);
            }
        }
        self.retire_untranslatable_to_interpreter(key);
    }

    fn retire_untranslatable_to_interpreter(&mut self, key: runtime::FunctionKey) {
        self.optimizing_snapshots.remove(&key);
        self.feedback_disabled.insert(key);
        self.tier2_untranslatable.insert(key);
        // A Baseline retained for a Tier2 trial is demoted here too.
        self.profitability_blacklisted.insert(key);
        self.coordinator.demote_baseline_to_interpreter(key);
    }

    fn maintenance(&mut self) {
        // Maintenance owns installation, demotion, feedback-epoch updates,
        // and reclamation. Invalidate before touching any of those states.
        self.invalidate_entry_cache();
        self.hot_ticks_since_maintenance = 0;
        self.last_scan_feedback_version = self.feedback.version();
        self.clock = self.clock.saturating_add(1);
        self.coordinator.advance_clock(self.clock);
        let installed_before = self.coordinator.metrics().installed;
        let install_started = std::time::Instant::now();
        self.coordinator.drain_completions();
        if self.coordinator.metrics().installed > installed_before {
            self.cold_metrics_dirty = true;
            self.install_ns = self.install_ns.saturating_add(
                install_started
                    .elapsed()
                    .as_nanos()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
        }
        let refresh_inputs = (self.feedback.version(), self.coordinator.installed_count());
        let baseline_only = self.config.tier_policy() == JitTierPolicy::BaselineOnly;
        let refresh_scan_due = self.last_refresh_scan != Some(refresh_inputs);
        if refresh_scan_due {
            self.last_refresh_scan = Some(refresh_inputs);
            // Under automatic tiering the baseline artifact is normally a
            // stepping stone and keeps its feedback-free helper sites. When
            // no feedback can make Tier 2 translate the function, baseline is
            // its terminal native tier: refresh it once with the recorded
            // (bounded polymorphic) property feedback, exactly as the
            // baseline-only policy does. Never race an active Tier-2 job.
            let candidates = if baseline_only {
                self.optimizing_snapshots
                    .keys()
                    .copied()
                    .collect::<Vec<_>>()
            } else {
                let mut keys = self
                    .optimizing_snapshots
                    .keys()
                    .chain(self.tier2_sources.keys())
                    .copied()
                    .filter(|key| !self.baseline_property_refreshed.contains(key))
                    .collect::<Vec<_>>();
                keys.sort_unstable_by_key(|key| (key.id, key.generation));
                keys.dedup();
                keys.retain(|key| {
                    !matches!(
                        self.coordinator.tier_state(*key, runtime::Tier::Optimizing),
                        runtime::CompileState::Queued(_)
                            | runtime::CompileState::Compiling(_)
                            | runtime::CompileState::Ready(_)
                            | runtime::CompileState::Installed(_)
                    )
                });
                let snapshots = &self.optimizing_snapshots;
                let sources = &self.tier2_sources;
                keys.retain(|key| {
                    let admitted = *self.optimized_vocabulary.entry(*key).or_insert_with(|| {
                        snapshots
                            .get(key)
                            .or_else(|| sources.get(key))
                            .is_none_or(ir::optimized_vocabulary_admits)
                    });
                    !admitted
                });
                // P2b x P4c: a generation that contains an opcode Tier 1 first
                // admitted in P2b and that Tier 2 cannot translate was
                // interpreter-only at 82d3808, and the optimizing scan settles
                // it back into the interpreter; its terminal tier is not
                // baseline, so it never takes a terminal refresh. Refreshing
                // it would race that demotion (an in-flight refresh leaves no
                // installed baseline to demote, and its later install keeps
                // the function native). Forced-trial test runs skip the
                // settle but still skip the refresh: recompiling such a
                // callee (for example an accessor reached from a frame-inlined
                // caller) re-publishes it mid-warmup and was observed to leave
                // the caller's Tier 2 artifact deoptimizing on an
                // object-typed guard until its side-path trial failed.
                let untranslatable = &self.tier2_untranslatable;
                keys.retain(|key| {
                    !untranslatable.contains(key)
                        && !snapshots
                            .get(key)
                            .or_else(|| sources.get(key))
                            .is_some_and(settles_untranslatable_candidate)
                });
                keys
            };
            // HERDR: longbridge/quickjs-jit#38. Copy the global call feedback
            // only for refreshes that queue, not for every rejected candidate.
            let refreshes = candidates
                .into_iter()
                .filter_map(|key| {
                    let installed = matches!(
                        self.coordinator.tier_state(key, runtime::Tier::Baseline),
                        runtime::CompileState::Installed(_)
                    );
                    if !baseline_only && !installed {
                        return None;
                    }
                    let snapshot = self
                        .optimizing_snapshots
                        .get(&key)
                        .or_else(|| self.tier2_sources.get(&key))?;
                    // Property admission does not need the global call tables.
                    let feedback = if baseline_only {
                        self.feedback.snapshot(self.clock.max(1))
                    } else {
                        runtime::FeedbackSnapshot::empty(self.clock.max(1))
                    }
                    .with_properties(self.shape_feedback.snapshot(key));
                    let call_ready = baseline_only
                        && self
                            .coordinator
                            .baseline_direct_refresh_ready(key, &feedback);
                    let property_ready = !self.baseline_property_refreshed.contains(&key)
                        && compiler::baseline::has_baseline_property_sites(snapshot, &feedback)
                        && installed;
                    (call_ready || property_ready).then(|| {
                        let feedback = if baseline_only {
                            feedback
                        } else {
                            self.feedback
                                .snapshot(self.clock.max(1))
                                .with_properties(self.shape_feedback.snapshot(key))
                        };
                        (key, snapshot.clone(), feedback, property_ready)
                    })
                })
                .collect::<Vec<_>>();
            for (key, snapshot, feedback, property_ready) in refreshes {
                if self.coordinator.prepare_baseline_direct_refresh(key)
                    && self
                        .coordinator
                        .queue_with_feedback(key, runtime::Tier::Baseline, snapshot, feedback)
                        .is_ok()
                    && property_ready
                {
                    self.baseline_property_refreshed.insert(key);
                }
            }
        }
        let ready_for_tier2 = self
            .optimizing_snapshots
            .keys()
            .copied()
            .filter(|key| {
                if self.feedback_disabled.contains(key) || self.tier2_untranslatable.contains(key) {
                    return false;
                }
                if self
                    .profitability_backoff
                    .get(key)
                    .is_some_and(|(_, retry_at)| self.clock < *retry_at)
                {
                    return false;
                }
                (matches!(
                    self.coordinator.tier_state(*key, runtime::Tier::Baseline),
                    runtime::CompileState::Installed(_)
                ) || self.profitability_blacklisted.contains(key))
                    && match self.coordinator.tier_state(*key, runtime::Tier::Optimizing) {
                        runtime::CompileState::Cold => true,
                        runtime::CompileState::Backoff { retry_after, .. } => {
                            self.clock >= retry_after
                        }
                        _ => false,
                    }
            })
            .collect::<Vec<_>>();
        let mut ready_for_tier2 = ready_for_tier2;
        if ready_for_tier2.len() > 1 {
            // Queue callees before their callers in one scan. A caller then
            // observes its callee's optimizing compile as in flight (below)
            // instead of freezing the callee's Baseline shape into its artifact.
            let calls = self.feedback.snapshot(self.clock.max(1));
            let ready = ready_for_tier2
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            ready_for_tier2.sort_by_cached_key(|key| {
                calls
                    .call_specializations_for(*key)
                    .filter(|call| call.callee() != *key && ready.contains(&call.callee()))
                    .count()
            });
        }
        self.last_scan_installed = self.coordinator.installed_count();
        for key in if self.config.tier_policy() == JitTierPolicy::BaselineOnly {
            Vec::new()
        } else {
            ready_for_tier2
        } {
            let Some(snapshot) = self.optimizing_snapshots.get(&key) else {
                continue;
            };
            #[cfg(feature = "test-support")]
            let forced_trial = self.config.force_optimized();
            #[cfg(not(feature = "test-support"))]
            let forced_trial = false;
            if !forced_trial && settles_untranslatable_candidate(snapshot) {
                self.settle_untranslatable_tier2_candidate(key);
                continue;
            }
            // Tier 2 has no exceptional control flow; exception regions stay
            // on their Tier 1 code instead of failing admission. Checked only
            // for ready candidates so the per-maintenance scan stays as is.
            // The P2b untranslatable settle above runs first, so a region
            // that also uses a newly admitted generic opcode still returns to
            // the interpreter exactly as that settle documents.
            if snapshot.has_exception_regions() {
                continue;
            }
            let scan_inputs = Tier2ScanInputs {
                feedback: self.feedback.version(),
                shapes: self.shape_feedback.version(),
                installed: self.coordinator.installed_count(),
                element_samples: self
                    .execution_profiles
                    .get(&key)
                    .map_or(0, |profile| profile.baseline_executions.min(8)),
            };
            if self.tier2_deferred.get(&key) == Some(&scan_inputs) {
                continue;
            }
            let observed = self
                .feedback
                .snapshot(self.clock.max(1))
                .with_properties(self.shape_feedback.snapshot(key));
            let mut direct_call_pending = false;
            // The callee-compile wait below depends on another function's
            // optimizing state, which `Tier2ScanInputs` does not capture: a
            // callee compile that fails or goes stale changes no scan input.
            // Such a deferral is transient and must not be cached.
            let mut callee_compile_wait = false;
            for instruction in snapshot.instructions() {
                let Some(call) = observed.call_specialization_at(key, instruction.pc()) else {
                    continue;
                };
                if call.callee() == key {
                    continue;
                }
                let repeated_in_loop = snapshot.instructions().iter().any(|branch| {
                    branch.pc() > instruction.pc()
                        && branch.branch_target().is_some_and(|target| {
                            u32::try_from(target).ok().is_some_and(|target| {
                                target <= instruction.pc()
                                    && snapshot.control_flow_graph().is_loop_header(target)
                            })
                        })
                });
                if !repeated_in_loop {
                    continue;
                }
                let direct_ready = self.coordinator.direct_call_ready(&call);
                let frame_ready = self.coordinator.frame_inline_candidate_ready(
                    key,
                    snapshot,
                    instruction.pc(),
                    &observed,
                );
                let resolved = self.coordinator.call_target_resolved(call.callee());
                let native_ready = self.coordinator.native_call_ready(call.callee());
                if direct_ready || native_ready {
                    continue;
                }
                if frame_ready {
                    /* A linked entry is published only by an artifact compiled
                     * with the callee's property feedback, which Baseline
                     * usually lacks. While that optimizing compile is in
                     * flight, wait for it rather than capture a frame inline
                     * of the Baseline body: the caller would never re-link.
                     * The wait ends when the compile installs or fails. */
                    let callee_optimizing = matches!(
                        self.coordinator
                            .tier_state(call.callee(), runtime::Tier::Optimizing),
                        runtime::CompileState::Queued(_)
                            | runtime::CompileState::Compiling(_)
                            | runtime::CompileState::Ready(_)
                    );
                    // Only a site the Tier 2 caller can link is worth the
                    // wait; any other site is frame-inlined regardless.
                    if callee_optimizing
                        && observed.bounded_specialization(call.callee()).is_some()
                        && runtime::tier2_direct_call_site_usable(snapshot, instruction.pc(), &call)
                    {
                        direct_call_pending = true;
                        callee_compile_wait = true;
                    }
                    continue;
                }
                // An admitted pure recursive callee publishes its native
                // entry with its optimizing artifact; wait for it as for a
                // compiling scalar callee. Like that wait, it depends on the
                // callee's optimizing state, which the scan inputs do not
                // capture, so it is rescanned instead of cached.
                if self.native_recursive.contains(&call.callee())
                    && matches!(
                        self.coordinator
                            .tier_state(call.callee(), runtime::Tier::Optimizing),
                        runtime::CompileState::Cold
                            | runtime::CompileState::Queued(_)
                            | runtime::CompileState::Compiling(_)
                            | runtime::CompileState::Ready(_)
                    )
                {
                    direct_call_pending = true;
                    callee_compile_wait = true;
                    continue;
                }
                if !resolved {
                    direct_call_pending = true;
                }
            }
            /* A caller queued while its monomorphic callee is still compiling
             * permanently lowers the site through the generic CALL bridge.
             * Keep the caller at its installed baseline until the callee's
             * scalar entry is publishable, then capture that pinned target in
             * the first Tier2 artifact. Self-recursive and non-specializable
             * calls keep their existing generic lowering. */
            if direct_call_pending {
                if callee_compile_wait {
                    self.tier2_deferred.remove(&key);
                } else {
                    self.tier2_deferred.insert(key, scan_inputs);
                }
                continue;
            }
            // A stable call link can fund a bounded optimizing trial before
            // every property IC in the same function has executed. The Tier-2
            // property lowering is intentionally typed and fail-closed: it
            // must not guess a shape or silently substitute an unpriced
            // helper path. As in V8/JSC, wait for each required property site
            // to acquire feedback, preserving the trial budget for a snapshot
            // whose IC dependencies can actually be compiled.
            let property_feedback_pending = snapshot.instructions().iter().any(|instruction| {
                matches!(instruction.opcode().name(), "get_field" | "put_field")
                    && observed.property_at(instruction.pc()).is_none()
            });
            if property_feedback_pending {
                self.tier2_deferred.insert(key, scan_inputs);
                continue;
            }
            /* A native-to-native call through the generic CALL bridge still
             * costs about ten interpreter calls (entry acquisition, hot
             * probing, timing and exit accounting run as callbacks per call).
             * A loop amortizes that against unboxed work; a function that
             * only calls (recursion, forwarding wrappers, tail calls) cannot,
             * and the cost model has no interpreter timing to notice that
             * both native tiers lose. Keep such functions in the interpreter
             * until the bridge is cached on the C side. */
            let has_loop = snapshot.control_flow_graph().blocks().iter().any(|block| {
                snapshot
                    .control_flow_graph()
                    .is_loop_header(block.start_pc())
            });
            /* A pure self-recursive Int32 function publishes a native call
             * entry (P3a): its monomorphic self calls run as direct native
             * calls without the generic bridge, so they no longer need the
             * loop amortization below. */
            let native_self_call = |pc: u32| {
                observed
                    .call_link_at(key, pc)
                    .is_some_and(|link| link.callee() == key)
            };
            let native_call_shape = !has_loop
                && snapshot
                    .instructions()
                    .iter()
                    .any(|instruction| native_self_call(instruction.pc()))
                && observed
                    .bounded_specialization(key)
                    .is_some_and(|signature| {
                        // Admission builds the entry's CLIF; reuse the answer
                        // while the function waits for its baseline profile.
                        // The feedback epoch changes every pass; the plan only
                        // depends on the representations.
                        match self.native_call_plans.get(&key) {
                            Some((arguments, result, admitted))
                                if **arguments == *signature.arguments()
                                    && *result == signature.result() =>
                            {
                                *admitted
                            }
                            _ => {
                                let admitted =
                                    compiler::native_call::plan(snapshot, &signature).is_some();
                                self.native_call_plans.insert(
                                    key,
                                    (signature.arguments().into(), signature.result(), admitted),
                                );
                                admitted
                            }
                        }
                    });
            // Entering native code from QuickJS still costs several hundred
            // nanoseconds of bookkeeping (B2), about ten interpreter calls.
            // Admit only recursion whose observed trees amortize it; while the
            // installed baseline is still collecting that profile, wait.
            let (baseline_executions, baseline_outermost) =
                self.execution_profiles.get(&key).map_or((0, 0), |profile| {
                    (profile.baseline_executions, profile.baseline_outermost)
                });
            let native_call_admitted = native_call_shape
                && baseline_executions >= NATIVE_RECURSION_PROFILE_EXECUTIONS
                && baseline_outermost != 0
                && baseline_executions
                    >= baseline_outermost.saturating_mul(NATIVE_RECURSION_MIN_CALLS_PER_ENTRY);
            // Trees already running in the interpreter when the baseline was
            // installed make early samples look shallow; reject only after
            // enough outside entries have been observed.
            if native_call_shape
                && !native_call_admitted
                && baseline_outermost < NATIVE_RECURSION_PROFILE_ENTRIES
                && matches!(
                    self.coordinator.tier_state(key, runtime::Tier::Baseline),
                    runtime::CompileState::Installed(_)
                )
            {
                continue;
            }
            if native_call_admitted {
                self.native_recursive.insert(key);
            }
            let generic_call_without_loop = !has_loop
                && snapshot.instructions().iter().any(|instruction| {
                    matches!(
                        instruction.opcode().name(),
                        "call"
                            | "call0"
                            | "call1"
                            | "call2"
                            | "call3"
                            | "call_method"
                            | "tail_call"
                            | "tail_call_method"
                            | "call_constructor"
                    ) && !observed
                        .call_specialization_at(key, instruction.pc())
                        .is_some_and(|call| {
                            call.callee() != key && self.coordinator.direct_call_ready(&call)
                        })
                        && !(native_call_admitted && native_self_call(instruction.pc()))
                        && !observed
                            .call_link_at(key, instruction.pc())
                            .is_some_and(|link| {
                                link.callee() != key
                                    && self.coordinator.native_call_ready(link.callee())
                            })
                });
            #[cfg(feature = "test-support")]
            let forced_call_only = self.config.force_optimized();
            #[cfg(not(feature = "test-support"))]
            let forced_call_only = false;
            if generic_call_without_loop && !forced_call_only {
                self.cold_metrics_dirty = true;
                self.generic_call_rejections = self.generic_call_rejections.saturating_add(1);
                self.optimizing_snapshots.remove(&key);
                self.feedback_disabled.insert(key);
                // A Baseline retained for a Tier2 trial is demoted here too.
                self.profitability_blacklisted.insert(key);
                self.coordinator.demote_baseline_to_interpreter(key);
                continue;
            }
            let numeric_candidate = snapshot.instructions().iter().any(|instruction| {
                matches!(instruction.opcode().name(), "add" | "sub" | "mul" | "div")
            }) && !snapshot.instructions().iter().any(|instruction| {
                // Every call form, including the short `call0`..`call3`
                // encodings: a caller's object or function arguments never
                // form a bounded numeric signature, so the wait below would
                // never end.
                matches!(
                    instruction.opcode().name(),
                    "call"
                        | "call0"
                        | "call1"
                        | "call2"
                        | "call3"
                        | "call_method"
                        | "tail_call"
                        | "tail_call_method"
                        | "call_constructor"
                        | "get_field"
                        | "put_field"
                )
            });
            /* A value-site observation can arrive at the loop hot probe before
             * the same invocation emits its CALL/RETURN feedback. Compiling at
             * that point freezes an `Any` numeric version and guarantees a
             * generic Float64 loop even when the completed invocation proves a
             * bounded Int32 signature. Keep the immutable bytecode snapshot,
             * but do not queue Tier2 until the signature is complete. A truly
             * feedback-free force-optimized test still uses its deterministic
             * fallback below; call/property candidates have their own bounded
             * identities and are not gated here. */
            if numeric_candidate
                && (observed.call_at(key).is_some() || observed.has_stable_value_for(key))
                && observed.bounded_specialization(key).is_none()
            {
                // Mixed Int32/Float64 array loops need not ever acquire one
                // uniform scalar signature. After complete baseline calls,
                // admit their existing guarded element lowering to the normal
                // profitability trial. Opcode presence alone is insufficient:
                // require observed object arguments and numeric returns too.
                let observed_element_loop = has_loop
                    && snapshot
                        .instructions()
                        .iter()
                        .any(|instruction| instruction.opcode().name() == "get_array_el")
                    && self
                        .execution_profiles
                        .get(&key)
                        .is_some_and(|profile| profile.baseline_executions >= 8)
                    && observed
                        .call_at(key)
                        .is_some_and(|call| call.state() == runtime::FeedbackState::Monomorphic)
                    && observed
                        .call_argument_types(key)
                        .is_some_and(|types| types.contains(&runtime::ObservedType::Object));
                let mut numeric_returns = observed
                    .entries()
                    .iter()
                    .filter(|entry| {
                        entry.function() == key && entry.kind() == runtime::FeedbackKind::Exit
                    })
                    .peekable();
                let completed_numeric_returns = numeric_returns.peek().is_some()
                    && numeric_returns.all(|entry| {
                        !entry.observations().is_empty()
                            && entry.observations().iter().all(|observed| {
                                matches!(
                                    observed,
                                    runtime::ObservedType::Int32 | runtime::ObservedType::Float64
                                )
                            })
                    });
                if !observed_element_loop || !completed_numeric_returns {
                    self.tier2_deferred.insert(key, scan_inputs);
                    continue;
                }
            }
            #[cfg(feature = "test-support")]
            let forced = self.config.force_optimized();
            #[cfg(not(feature = "test-support"))]
            let forced = false;
            // As in V8/JSC, stable IC feedback is itself sufficient to fund
            // one bounded optimizing trial. Baseline timing cannot model the
            // compiled call/property/element fast path, so rejecting it here
            // would make the optimized path permanently unreachable. The
            // measured Tier-2 window below still demotes a losing artifact.
            let stable_ic_candidate = native_call_admitted
                || snapshot.instructions().iter().any(|instruction| {
                    observed
                        .call_specialization_at(key, instruction.pc())
                        .is_some_and(|call| call.callee() != key)
                        || observed.property_at(instruction.pc()).is_some()
                        || observed
                            .array_at(key, instruction.pc())
                            .is_some_and(|site| site.can_specialize())
                });
            /* A rejected baseline does not predict Tier2 profitability: the
             * baseline can lose to dispatch/callback overhead while a stable
             * unboxed loop wins by orders of magnitude.  Give such a function
             * one bounded optimizing trial; coordinator attempt/version caps
             * still prevent compile or deopt loops. */
            /* Baseline measurements cannot price the compiled-to-compiled
             * transition: before the callee is publishable they include the
             * generic CALL bridge that Tier2 removes. Once every stable
             * direct target is ready, admit the caller's existing bounded
             * optimizing trial without spending five misleading baseline
             * profitability retries. */
            if !forced && !stable_ic_candidate && !self.profitability_blacklisted.contains(&key) {
                let measured = self
                    .execution_profiles
                    .get(&key)
                    .copied()
                    .unwrap_or_default();
                let installed = self.coordinator.metrics().installed.max(1);
                let profile = runtime::Profile {
                    bytecodes: measured.bytecodes,
                    helper_calls: measured.helper_calls,
                    compile_ns: self
                        .compiler_measurements
                        .elapsed_ns
                        .load(Ordering::Relaxed)
                        / installed,
                    install_ns: self.install_ns / installed,
                    executions: measured.baseline_executions,
                    baseline_ns: measured.baseline_ns,
                    code_bytes: measured.bytecodes.saturating_mul(8),
                    ..runtime::Profile::default()
                };
                self.cold_metrics_dirty = true;
                self.profitability_evaluations = self.profitability_evaluations.saturating_add(1);
                if runtime::Profitability::default()
                    .evaluate_trial(profile)
                    .tier
                    != runtime::Decision::Optimize
                {
                    self.cold_metrics_dirty = true;
                    self.profitability_rejected = self.profitability_rejected.saturating_add(1);
                    let entry = self
                        .profitability_backoff
                        .entry(key)
                        .or_insert((0, self.clock));
                    entry.0 = entry.0.saturating_add(1);
                    if entry.0 >= 5 {
                        // Queue the funded trial at the next maintenance
                        // instead of the next periodic one.
                        entry.1 = self.clock.saturating_add(1);
                        self.next_profitability_retry = self.next_profitability_retry.min(entry.1);
                        // Exhausted retries fund one bounded Tier2 trial
                        // (below, through the blacklist). Only a Baseline
                        // whose entry cost can dominate its body goes back to
                        // the interpreter meanwhile; demoting a long-running
                        // one would stall it until the queued trial installs.
                        self.profitability_blacklisted.insert(key);
                        // No complete execution yet: keep the conservative
                        // demotion.
                        if measured.baseline_min_ns == 0
                            || measured.baseline_min_ns < ENTRY_DOMINATED_BASELINE_NS
                        {
                            self.coordinator.demote_baseline_to_interpreter(key);
                        }
                    } else {
                        entry.1 = self.clock.saturating_add(1u64 << entry.0.min(20));
                        self.next_profitability_retry = self.next_profitability_retry.min(entry.1);
                    }
                    continue;
                }
                self.profitability_backoff.remove(&key);
                self.cold_metrics_dirty = true;
                self.profitability_approved = self.profitability_approved.saturating_add(1);
            } else if !forced && stable_ic_candidate {
                self.cold_metrics_dirty = true;
                self.profitability_evaluations = self.profitability_evaluations.saturating_add(1);
                self.profitability_approved = self.profitability_approved.saturating_add(1);
            }
            if let Some(snapshot) = self.optimizing_snapshots.remove(&key) {
                #[cfg(feature = "test-support")]
                let feedback = if self.config.force_optimized() {
                    let has_stable_call = snapshot.instructions().iter().any(|instruction| {
                        observed
                            .call_specialization_at(key, instruction.pc())
                            .is_some()
                    });
                    let has_property = snapshot
                        .instructions()
                        .iter()
                        .any(|i| observed.property_at(i.pc()).is_some());
                    if observed.bounded_specialization(key).is_some()
                        || snapshot
                            .instructions()
                            .iter()
                            .any(|i| observed.array_at(key, i.pc()).is_some())
                        || has_stable_call
                        || has_property
                    {
                        observed
                    } else {
                        let mut deterministic = runtime::FeedbackTable::new(1, 1);
                        deterministic.observe_type(
                            key,
                            0,
                            runtime::FeedbackKind::Value,
                            runtime::ObservedType::Int32,
                        );
                        deterministic.snapshot(self.clock.max(1))
                    }
                } else {
                    self.feedback
                        .snapshot(self.clock)
                        .with_properties(self.shape_feedback.snapshot(key))
                };
                #[cfg(not(feature = "test-support"))]
                let feedback = self
                    .feedback
                    .snapshot(self.clock)
                    .with_properties(self.shape_feedback.snapshot(key));
                if self
                    .coordinator
                    .queue_with_feedback(key, runtime::Tier::Optimizing, snapshot.clone(), feedback)
                    .is_err()
                {
                    self.optimizing_snapshots.insert(key, snapshot);
                } else {
                    // A trial funded by exhausted retries while its
                    // long-running Baseline was retained: the function is
                    // proven hot and slow, so compile it ahead of first
                    // Baselines queued by other functions.
                    if self.profitability_blacklisted.contains(&key)
                        && matches!(
                            self.coordinator.tier_state(key, runtime::Tier::Baseline),
                            runtime::CompileState::Installed(_)
                        )
                    {
                        self.coordinator.prioritize(key, runtime::Tier::Optimizing);
                    }
                    self.tier2_deferred.remove(&key);
                    self.tier2_sources.insert(key, snapshot);
                }
            }
        }
        self.workers.drain_overflow(
            &mut self.coordinator,
            runtime::DEFAULT_COMPLETION_DRAIN_BUDGET,
        );
        while matches!(self.workers.dispatch_next(&mut self.coordinator), Ok(true)) {}
        let (jobs, snapshots, ir) = self.workers.live_usage();
        self.cold_metrics_dirty = true;
        self.peak_compiler_bytes = self
            .peak_compiler_bytes
            .max(self.workers.peak_compiler_bytes());
        self.coordinator.set_worker_usage(jobs, snapshots, ir);
        let requested = self.requested.iter().copied().collect::<Vec<_>>();
        for key in requested {
            let retryable = match self.coordinator.tier_state(key, runtime::Tier::Baseline) {
                runtime::CompileState::Cold | runtime::CompileState::Retired => true,
                runtime::CompileState::Backoff { retry_after, .. } => self.clock >= retry_after,
                _ => false,
            };
            if retryable {
                self.clear_failed_request(key);
            }
        }
        self.publish_metrics();
    }

    /// Copies the backend's counters into the externally visible metrics
    /// snapshot. Native and asynchronous counters remain exact on every exit;
    /// admission and compilation counters are copied only after a mutation.
    fn publish_metrics(&mut self) {
        let mut snapshot = self.metrics.lock().unwrap_or_else(|p| p.into_inner());
        let replaced = self.coordinator.refresh_published_metrics(&mut snapshot);
        snapshot.native_entries = self.native_entries;
        snapshot.native_acquisitions = self.native_acquisitions;
        snapshot.native_exits = self.native_exits;
        snapshot.native_fallbacks = self.native_fallbacks;
        snapshot.native_retries = self.native_retries;
        self.osr_entries = self.osr_validation.successes.load(Ordering::Relaxed);
        self.osr_validation_failures = self.osr_validation.failures.load(Ordering::Relaxed);
        snapshot.osr_entries = self.osr_entries;
        snapshot.osr_attempts = self.osr_attempts;
        snapshot.osr_validated_successes = self.osr_validation.successes.load(Ordering::Relaxed);
        snapshot.osr_generated_retries = self.osr_generated_retries;
        // A full coordinator copy also clears backend-owned fields. Restore
        // cold counters in that case even when they have not changed locally.
        if replaced || self.cold_metrics_dirty {
            snapshot.hot_call_queues = self.hot_call_queues;
            snapshot.hot_loop_queues = self.hot_loop_queues;
            snapshot.adaptive_neutral_queues = self.adaptive_neutral_queues;
            snapshot.adaptive_inputs_recorded = self.adaptive_inputs_recorded;
            snapshot.adaptive_size_factor_disabled = self.adaptive_inputs_recorded;
            snapshot.snapshot_requests = self.snapshot_requests;
            snapshot.stable_path_compile_requests = self.stable_path_compile_requests;
            snapshot.generic_call_rejections = self.generic_call_rejections;
            snapshot.profitability_evaluations = self.profitability_evaluations;
            snapshot.profitability_approved = self.profitability_approved;
            snapshot.profitability_rejected = self.profitability_rejected;
            snapshot.install_ns = self.install_ns;
            snapshot.peak_compiler_bytes = self.peak_compiler_bytes;
            self.cold_metrics_dirty = false;
        }
        snapshot.benefit_recordings = self.benefit_recordings;
        snapshot.measured_benefit_ns = self.measured_benefit_ns;
        snapshot.compile_ns = self
            .compiler_measurements
            .elapsed_ns
            .load(Ordering::Relaxed);
        snapshot.osr_not_ready = self.osr_not_ready;
        snapshot.osr_map_misses = self.osr_map_misses;
        snapshot.osr_validation_failures = self.osr_validation_failures;
        snapshot.deopt_materializations = self
            .osr_validation
            .deopt_materializations
            .load(Ordering::Relaxed);
    }

    fn clear_failed_request(&mut self, key: runtime::FunctionKey) {
        self.requested.remove(&key);
        self.queue_reasons.remove(&key);
        if let Some(hotness) = self.hotness.get_mut(&key) {
            hotness.clear_queued();
        }
    }

    fn fail_before_queue(&mut self, key: runtime::FunctionKey) {
        let attempts = self
            .prequeue_backoff
            .get(&key)
            .map_or(1, |(attempts, _)| attempts.saturating_add(1));
        let delay = 1_u64 << u32::from(attempts.min(16));
        self.prequeue_backoff
            .insert(key, (attempts, self.clock.saturating_add(delay)));
        self.clear_failed_request(key);
    }
}

#[cfg(all(
    feature = "compiler",
    any(
        all(
            target_os = "macos",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "windows",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "linux",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )
))]
unsafe impl rquickjs_core::runtime::JitBackend for ProductionBackend {
    fn poll(&mut self) {
        // HERDR: rquickjs polls before and after every pending job. A full pass
        // each time cloned the whole feedback table hundreds of times a second
        // in busy promise loops; the gate still runs it for pending compiles.
        self.maintenance_if_due(false);
    }

    fn record_hot(&mut self, event: &rquickjs_core::qjs::JSJitHotEvent) -> u32 {
        if event.count == 0 {
            return 0;
        }
        let key = runtime::FunctionKey::new(event.function.id, event.function.generation);
        /* Bit 1 is an append-only response flag understood by the patched
         * interpreter: this immutable bytecode generation will never acquire
         * code, so stop hot and feedback probes locally in C. */
        if self.feedback_disabled.contains(&key) || self.coordinator.is_terminally_blacklisted(key)
        {
            self.feedback_disabled.insert(key);
            return 2;
        }
        self.maintenance_if_due(false);
        // Maintenance may publish code. Feedback below cannot change this state;
        // a successful direct refresh returns before the cached value is reused.
        let baseline_state = self.coordinator.tier_state(key, runtime::Tier::Baseline);
        let observed = match event.feedback_type {
            rquickjs_core::qjs::JSJitFeedbackType_JS_JIT_FEEDBACK_INT32 => {
                Some(runtime::ObservedType::Int32)
            }
            rquickjs_core::qjs::JSJitFeedbackType_JS_JIT_FEEDBACK_FLOAT64 => {
                Some(runtime::ObservedType::Float64)
            }
            rquickjs_core::qjs::JSJitFeedbackType_JS_JIT_FEEDBACK_BOOL => {
                Some(runtime::ObservedType::Bool)
            }
            rquickjs_core::qjs::JSJitFeedbackType_JS_JIT_FEEDBACK_NULL => {
                Some(runtime::ObservedType::Null)
            }
            rquickjs_core::qjs::JSJitFeedbackType_JS_JIT_FEEDBACK_UNDEFINED => {
                Some(runtime::ObservedType::Undefined)
            }
            rquickjs_core::qjs::JSJitFeedbackType_JS_JIT_FEEDBACK_STRING => {
                Some(runtime::ObservedType::String)
            }
            rquickjs_core::qjs::JSJitFeedbackType_JS_JIT_FEEDBACK_OBJECT => {
                Some(runtime::ObservedType::Object)
            }
            _ => None,
        };
        if event.feedback_slot != u32::MAX {
            if let Some(observed) = observed {
                self.feedback
                    .observe_type(key, event.pc, runtime::FeedbackKind::Value, observed);
            }
        }
        if event.callee.id != 0 {
            self.feedback.observe_type(
                key,
                event.pc,
                runtime::FeedbackKind::CallTarget,
                runtime::ObservedType::Function(runtime::FunctionKey::new(
                    event.callee.id,
                    event.callee.generation,
                )),
            );
        }
        let probe_inputs = (self.feedback.version(), self.coordinator.installed_count());
        if self.config.tier_policy() == JitTierPolicy::BaselineOnly
            && matches!(baseline_state, runtime::CompileState::Installed(_))
            && self.direct_refresh_probes.get(&key) != Some(&probe_inputs)
        {
            // The readiness answer depends only on the feedback lattice and
            // the installed artifact set; re-probe when either changed.
            self.direct_refresh_probes.insert(key, probe_inputs);
            let feedback = self.feedback.snapshot(self.clock);
            if self
                .coordinator
                .baseline_direct_refresh_ready(key, &feedback)
                && self.coordinator.prepare_baseline_direct_refresh(key)
            {
                self.invalidate_entry_cache();
                self.requested.insert(key);
                self.queue_reasons
                    .insert(key, runtime::HotReason::CallThreshold);
                self.cold_metrics_dirty = true;
                self.snapshot_requests = self.snapshot_requests.saturating_add(1);
                return 1;
            }
        }
        if matches!(baseline_state, runtime::CompileState::Installed(_))
            || self.profitability_blacklisted.contains(&key)
        {
            let optimizing_ready = match self.coordinator.tier_state(key, runtime::Tier::Optimizing)
            {
                runtime::CompileState::Cold => true,
                runtime::CompileState::Backoff { retry_after, .. } => self.clock >= retry_after,
                _ => false,
            };
            if !optimizing_ready
                || self.optimizing_requested.contains(&key)
                || self.optimizing_snapshots.contains_key(&key)
            {
                return 0;
            }
            let thresholds = runtime::HotThresholds {
                calls: self.config.call_threshold(),
                loops: self.config.loop_threshold(),
                rationale: runtime::HotReason::NeutralBase,
            };
            let hotness = self.optimizing_hotness.entry(key).or_default();
            let decision = if event.kind == rquickjs_core::qjs::JSJitHotKind_JS_JIT_HOT_CALL
                && event.pc == 0
            {
                hotness.record_call_event_with_thresholds(event.count, thresholds)
            } else if event.kind == rquickjs_core::qjs::JSJitHotKind_JS_JIT_HOT_LOOP
                && event.pc != 0
            {
                hotness.record_loop_event_with_thresholds(event.count, thresholds)
            } else {
                return 0;
            };
            if matches!(decision, runtime::HotDecision::Queue(_)) {
                self.optimizing_requested.insert(key);
                self.cold_metrics_dirty = true;
                self.snapshot_requests = self.snapshot_requests.saturating_add(1);
                return 1;
            }
            return 0;
        }
        if matches!(
            baseline_state,
            runtime::CompileState::Blacklisted | runtime::CompileState::Installed(_)
        ) {
            self.requested.insert(key);
            return 0;
        }
        if self
            .prequeue_backoff
            .get(&key)
            .is_some_and(|(_, retry_after)| self.clock < *retry_after)
        {
            return 0;
        }
        if self.requested.contains(&key) {
            return 0;
        }
        let hotness = self.hotness.entry(key).or_default();
        // Task 14 owns evidence-based coefficients. Production still consumes
        // the adaptive interface now, with its documented neutral inputs.
        let adaptive = runtime::AdaptiveInputs::default().thresholds();
        let thresholds = runtime::HotThresholds {
            calls: self.config.call_threshold(),
            loops: self.config.loop_threshold(),
            rationale: adaptive.rationale,
        };
        let decision = if event.kind == rquickjs_core::qjs::JSJitHotKind_JS_JIT_HOT_CALL
            && event.pc == 0
        {
            hotness.record_call_event_with_thresholds(event.count, thresholds)
        } else if event.kind == rquickjs_core::qjs::JSJitHotKind_JS_JIT_HOT_LOOP && event.pc != 0 {
            hotness.record_loop_event_with_thresholds(event.count, thresholds)
        } else {
            return 0;
        };
        if let runtime::HotDecision::Queue(reason) = decision {
            self.requested.insert(key);
            self.queue_reasons.insert(key, reason);
            self.cold_metrics_dirty = true;
            self.snapshot_requests = self.snapshot_requests.saturating_add(1);
            1
        } else {
            0
        }
    }

    fn record_feedback(&mut self, event: &rquickjs_core::qjs::JSJitFeedbackEvent) {
        use rquickjs_core::qjs;

        if event.struct_size < core::mem::size_of::<qjs::JSJitFeedbackEvent>() as u32
            || event.function.id == 0
            || event.function.generation == 0
            || (event.type_count != 0 && event.types.is_null())
        {
            return;
        }
        let key = runtime::FunctionKey::new(event.function.id, event.function.generation);
        if self.feedback_disabled.contains(&key) {
            return;
        }
        let raw_types = if event.type_count == 0 {
            &[][..]
        } else {
            // QuickJS guarantees this borrowed array remains live for the callback.
            unsafe { core::slice::from_raw_parts(event.types, event.type_count as usize) }
        };
        let observed = |raw| match raw {
            qjs::JSJitValueType_JS_JIT_VALUE_INT32 => Some(runtime::ObservedType::Int32),
            qjs::JSJitValueType_JS_JIT_VALUE_FLOAT64 => Some(runtime::ObservedType::Float64),
            qjs::JSJitValueType_JS_JIT_VALUE_BOOL => Some(runtime::ObservedType::Bool),
            qjs::JSJitValueType_JS_JIT_VALUE_NULL => Some(runtime::ObservedType::Null),
            qjs::JSJitValueType_JS_JIT_VALUE_UNDEFINED => Some(runtime::ObservedType::Undefined),
            qjs::JSJitValueType_JS_JIT_VALUE_STRING => Some(runtime::ObservedType::String),
            qjs::JSJitValueType_JS_JIT_VALUE_OBJECT => Some(runtime::ObservedType::Object),
            qjs::JSJitValueType_JS_JIT_VALUE_BIGINT => Some(runtime::ObservedType::BigInt),
            qjs::JSJitValueType_JS_JIT_VALUE_SYMBOL => Some(runtime::ObservedType::Symbol),
            _ => None,
        };
        self.clock = self.clock.saturating_add(1).max(1);
        match event.kind {
            qjs::JSJitFeedbackKind_JS_JIT_FEEDBACK_ARRAY if raw_types.is_empty() => {
                self.feedback
                    .observe_array_raw(key, event.pc, event.slot, event.flags);
            }
            qjs::JSJitFeedbackKind_JS_JIT_FEEDBACK_CALL => {
                let arguments = &mut self.call_feedback_types;
                arguments.clear();
                arguments.reserve(raw_types.len());
                for &raw in raw_types {
                    let Some(value) = observed(raw) else {
                        // Reject the entire event before mutating feedback.
                        return;
                    };
                    arguments.push(value);
                }
                if event.flags & qjs::JS_JIT_FEEDBACK_CALL_SITE != 0 {
                    let Some((&result, arguments)) = arguments.split_last() else {
                        return;
                    };
                    if event.callee.id == 0 || event.callee.generation == 0 {
                        return;
                    }
                    let callee =
                        runtime::FunctionKey::new(event.callee.id, event.callee.generation);
                    self.feedback.observe_call_signature_with_identity(
                        key,
                        event.pc,
                        callee,
                        event.shape_identity,
                        event.prototype_identity,
                        arguments,
                        result,
                    );
                } else {
                    self.feedback.observe_call(key, arguments);
                }
            }
            qjs::JSJitFeedbackKind_JS_JIT_FEEDBACK_RETURN if raw_types.len() == 1 => {
                if let Some(result) = observed(raw_types[0]) {
                    self.feedback.observe_return(key, event.pc, result);
                }
            }
            qjs::JSJitFeedbackKind_JS_JIT_FEEDBACK_BINARY if raw_types.len() == 3 => {
                if let (Some(lhs), Some(rhs), Some(result)) = (
                    observed(raw_types[0]),
                    observed(raw_types[1]),
                    observed(raw_types[2]),
                ) {
                    let mut flags = runtime::BinaryFeedbackFlags::NONE;
                    if event.flags & qjs::JS_JIT_FEEDBACK_OVERFLOW != 0 {
                        flags |= runtime::BinaryFeedbackFlags::OVERFLOW;
                    }
                    if event.flags & qjs::JS_JIT_FEEDBACK_NEGATIVE_ZERO != 0 {
                        flags |= runtime::BinaryFeedbackFlags::NEGATIVE_ZERO;
                    }
                    if event.flags & qjs::JS_JIT_FEEDBACK_NAN != 0 {
                        flags |= runtime::BinaryFeedbackFlags::NAN;
                    }
                    self.feedback
                        .observe_binary(key, event.pc, lhs, rhs, result, flags);
                }
            }
            qjs::JSJitFeedbackKind_JS_JIT_FEEDBACK_CONVERSION if raw_types.len() == 2 => {
                if let (Some(operand), Some(result)) =
                    (observed(raw_types[0]), observed(raw_types[1]))
                {
                    self.feedback
                        .observe_conversion(key, event.pc, operand, result);
                }
            }
            qjs::JSJitFeedbackKind_JS_JIT_FEEDBACK_BRANCH if raw_types.len() == 1 => {
                if let Some(condition) = observed(raw_types[0]) {
                    self.feedback.observe_branch(
                        key,
                        event.pc,
                        condition,
                        event.flags & qjs::JS_JIT_FEEDBACK_BRANCH_TAKEN != 0,
                    );
                }
            }
            qjs::JSJitFeedbackKind_JS_JIT_FEEDBACK_PROPERTY
                if raw_types.len() == 2
                    && event.shape_identity != 0
                    && event.shape_generation != 0 =>
            {
                let observation = runtime::ShapeObservation::new(
                    runtime::ShapeToken::new(event.shape_identity, event.shape_generation),
                    runtime::PrototypeDependencyToken::new(
                        event.prototype_identity,
                        event.prototype_generation,
                    ),
                    event.property_offset,
                    runtime::PropertyAttributes::from_bits(event.property_attributes as u8),
                    observed(raw_types[1]).unwrap_or(runtime::ObservedType::Object),
                );
                self.shape_feedback.observe(key, event.pc, observation);
            }
            _ => {}
        }
    }

    fn submit_snapshot(&mut self, snapshot: *mut rquickjs_core::qjs::JSJitFunctionSnapshot) {
        struct FreeSnapshot(*mut rquickjs_core::qjs::JSJitFunctionSnapshot);
        impl Drop for FreeSnapshot {
            fn drop(&mut self) {
                unsafe { rquickjs_core::qjs::JS_JitFreeSnapshot(self.0) };
            }
        }
        let Some(raw) = std::ptr::NonNull::new(snapshot) else {
            return;
        };
        let raw_key = unsafe {
            runtime::FunctionKey::new(raw.as_ref().function.id, raw.as_ref().function.generation)
        };
        let _free = FreeSnapshot(snapshot);
        let copied = unsafe { bytecode::CompileSnapshot::copy_borrowed_raw(raw.as_ref()) };
        let Ok(snapshot) = copied else {
            self.feedback_disabled.insert(raw_key);
            self.fail_before_queue(raw_key);
            return;
        };
        let key = runtime::FunctionKey::new(snapshot.function_id(), snapshot.generation());
        if snapshot.owned_bytes() > self.config.max_snapshot_bytes() {
            // The configured copy budget is a transient admission failure, not
            // a property of this immutable bytecode generation.  Keep hot
            // probes enabled so `fail_before_queue` can perform its bounded
            // prequeue retries.
            self.coordinator.record_resource_limit_rejection();
            self.fail_before_queue(key);
            self.maintenance();
            return;
        }
        let Ok(verified) = snapshot.verify(bytecode::VerifyLimits::default()) else {
            self.feedback_disabled.insert(key);
            self.fail_before_queue(key);
            return;
        };
        if let Err(rejection) = verified.tier1_eligibility() {
            self.coordinator
                .reject_tier1(key, runtime::Tier::Baseline, rejection.reason());
            self.feedback_disabled.insert(key);
            self.clear_failed_request(key);
            self.maintenance();
            return;
        }
        if automatic_closure_creation_unprofitable(&self.config, &verified) {
            self.coordinator.reject_tier1(
                key,
                runtime::Tier::Baseline,
                bytecode::FallbackReason::ClosureFrame,
            );
            self.feedback_disabled.insert(key);
            self.clear_failed_request(key);
            self.maintenance();
            return;
        }
        if let Some(reason) = automatic_deferred_tier1_rejection(&self.config, &verified) {
            self.coordinator
                .reject_tier1(key, runtime::Tier::Baseline, reason);
            self.feedback_disabled.insert(key);
            self.clear_failed_request(key);
            self.maintenance();
            return;
        }
        self.ensure_artifact_environment();
        let adaptive = runtime::AdaptiveInputs {
            bytecode_bytes: verified
                .snapshot()
                .bytecode()
                .len()
                .try_into()
                .unwrap_or(u32::MAX),
            helper_ops: 0,
            instruction_count: verified.instructions().len().try_into().unwrap_or(u32::MAX),
            measured_work: None,
        };
        let profile = self.execution_profiles.entry(key).or_default();
        profile.bytecodes = verified.instructions().len().try_into().unwrap_or(u64::MAX);
        profile.helper_calls = verified
            .instructions()
            .iter()
            .filter(|instruction| {
                matches!(
                    bytecode::tier1_policy(instruction.opcode().id()),
                    Some(bytecode::Tier1Policy::Helper(_))
                )
            })
            .count()
            .try_into()
            .unwrap_or(u64::MAX);
        self.cold_metrics_dirty = true;
        self.adaptive_inputs_recorded = self.adaptive_inputs_recorded.saturating_add(1);
        debug_assert_eq!(
            adaptive.thresholds().rationale,
            runtime::HotReason::NeutralBase
        );
        if self.optimizing_requested.remove(&key) {
            if let Some(hotness) = self.optimizing_hotness.get_mut(&key) {
                hotness.clear_queued();
            }
            /* Optimizing snapshots must acquire the current bounded feedback
             * in maintenance. Directly queueing this callback result uses the
             * feedback-free coordinator path and races the RETURN event that
             * completes the numeric signature. */
            self.optimizing_snapshots.insert(key, verified);
            self.maintenance();
            return;
        }
        let requested_tier = runtime::Tier::Baseline;
        let tier2_snapshot = Some(verified.clone());
        let feedback = self.feedback.snapshot(self.clock);
        let queued = self
            .coordinator
            .queue_with_feedback(key, requested_tier, verified, feedback);
        match queued {
            Ok(()) => {
                match self.queue_reasons.get(&key).copied() {
                    Some(runtime::HotReason::CallThreshold) => {
                        self.cold_metrics_dirty = true;
                        self.hot_call_queues = self.hot_call_queues.saturating_add(1);
                    }
                    Some(runtime::HotReason::LoopThreshold) => {
                        self.cold_metrics_dirty = true;
                        self.hot_loop_queues = self.hot_loop_queues.saturating_add(1);
                    }
                    _ => {}
                }
                self.cold_metrics_dirty = true;
                self.adaptive_neutral_queues = self.adaptive_neutral_queues.saturating_add(1);
                self.prequeue_backoff.remove(&key);
                if let Some(snapshot) = tier2_snapshot {
                    self.optimizing_snapshots.insert(key, snapshot);
                }
            }
            Err(runtime::QueueError::Blacklisted | runtime::QueueError::NotReady) => {
                // An active/installed/blacklisted coordinator state owns the
                // request bit; retaining it prevents duplicate snapshots.
                self.requested.insert(key);
            }
            Err(runtime::QueueError::Retired | runtime::QueueError::Shutdown) => {
                self.clear_failed_request(key);
            }
            Err(_) => self.fail_before_queue(key),
        }
        self.maintenance();
    }

    fn acquire_entry(
        &mut self,
        id: u64,
        generation: u64,
        pc: u32,
    ) -> rquickjs_core::qjs::JSJitEntryHandle {
        let mut empty = rquickjs_core::qjs::JSJitEntryHandle {
            struct_size: core::mem::size_of::<rquickjs_core::qjs::JSJitEntryHandle>() as u32,
            reserved: 0,
            entry: None,
            pin: core::ptr::null_mut(),
            stack_map_count: 0,
            helper_abi_version: 0,
        };
        let key = runtime::FunctionKey::new(id, generation);
        if self.feedback_disabled.contains(&key) {
            return empty;
        }
        // Profitability-demoted functions have no publishable native entry.
        // Avoid running global maintenance at every subsequent call/OSR probe;
        // this keeps the fail-open interpreter path close to unattached cost.
        // A blacklisted function whose long-running Baseline was retained for
        // its Tier2 trial still enters that Baseline.
        if self.profitability_blacklisted.contains(&key)
            && !matches!(
                self.coordinator.tier_state(key, runtime::Tier::Optimizing),
                runtime::CompileState::Installed(_)
            )
            && !matches!(
                self.coordinator.tier_state(key, runtime::Tier::Baseline),
                runtime::CompileState::Installed(_)
            )
        {
            return empty;
        }
        self.maintenance_if_due(false);
        let acquired_tier = if matches!(
            self.coordinator.tier_state(key, runtime::Tier::Optimizing),
            runtime::CompileState::Installed(_)
        ) {
            runtime::Tier::Optimizing
        } else {
            runtime::Tier::Baseline
        };
        let Some(pin) = self.coordinator.pin(key, acquired_tier) else {
            if pc != 0 {
                self.osr_not_ready = self.osr_not_ready.saturating_add(1);
            }
            return empty;
        };
        let Some(published) = pin.artifact().published() else {
            if pc != 0 {
                self.osr_not_ready = self.osr_not_ready.saturating_add(1);
            }
            return empty;
        };
        let (published, osr_map) = if pc == 0 {
            (published, None)
        } else {
            let Some((map, osr)) = published.osr_entry(pc) else {
                self.osr_map_misses = self.osr_map_misses.saturating_add(1);
                return empty;
            };
            if map.key().function() != runtime::FunctionKey::new(id, generation) {
                self.osr_map_misses = self.osr_map_misses.saturating_add(1);
                return empty;
            }
            (osr, Some(map.clone()))
        };
        #[cfg(feature = "test-support")]
        {
            *self
                .test_last_acquired_key
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(pin.artifact().key());
        }
        let entry = published.as_ptr();
        let stack_map_count = published.required_stack_map_count();
        let artifact_key = pin.artifact().key();
        let artifact = pin.artifact();
        let snapshot_counts = artifact.inline_snapshot().map(|snapshot| {
            (
                usize::from(snapshot.arg_count()),
                usize::from(snapshot.local_count()),
            )
        });
        let deopt_sites = artifact
            .optimized_metadata()
            .map_or(&[][..], |metadata| metadata.deopt_sites());
        let osr_counts = osr_map.as_ref().map(|map| {
            (
                usize::from(map.argument_count()),
                usize::from(map.local_count()),
            )
        });
        let frame_counts = precomputed_frame_counts(snapshot_counts, deopt_sites, osr_counts);
        let pin = Box::into_raw(Box::new(ProductionEntryPin {
            execution: pin,
            native: entry,
            runtime_id: artifact_key.runtime_id,
            key: runtime::FunctionKey::new(id, generation),
            pc,
            stack_map_count,
            osr: osr_map,
            frame_counts,
            validation: Arc::clone(&self.osr_validation),
            #[cfg(feature = "test-support")]
            stress_gc: self.config.stress_gc(),
        }))
        .cast();
        empty.entry = Some(production_entry_trampoline);
        empty.pin = pin;
        empty.stack_map_count = stack_map_count;
        empty.helper_abi_version = rquickjs_core::qjs::QJSJIT_HELPER_ABI_VERSION;
        self.entry_tiers.insert(key, acquired_tier);
        self.native_acquisitions = self.native_acquisitions.saturating_add(1);
        empty
    }

    fn entry_cache_epoch(&self) -> u64 {
        if self.coordinator.has_pending_work()
            || self.feedback.version() != self.last_scan_feedback_version
            || self.coordinator.installed_count() != self.last_scan_installed
        {
            // Force acquisition through maintenance when the admission inputs
            // changed since the last scan, even before the next hot probe.
            0
        } else {
            self.entry_cache_epoch
        }
    }

    fn release_entry(&mut self, entry: rquickjs_core::qjs::JSJitEntryHandle) {
        if !entry.pin.is_null() {
            unsafe { drop(Box::from_raw(entry.pin.cast::<ProductionEntryPin>())) };
        }
    }

    fn native_enter(&mut self, _id: u64, _generation: u64, pc: u32) {
        self.native_entries = self.native_entries.saturating_add(1);
        let key = runtime::FunctionKey::new(_id, _generation);
        let tier = self
            .entry_tiers
            .get(&key)
            .copied()
            .unwrap_or(runtime::Tier::Baseline);
        if tier == runtime::Tier::Optimizing {
            self.coordinator.record_tier2_entry();
        }
        if pc != 0 {
            self.osr_attempts = self.osr_attempts.saturating_add(1);
        }
        // Counted at entry: a recursion tree's root is still active while
        // maintenance samples its completed nested executions. Profiles are
        // created by the first exit; entry never allocates.
        if tier == runtime::Tier::Baseline
            && self
                .execution_starts
                .last()
                .is_none_or(|(parent, _, _, _)| *parent != key)
        {
            if let Some(profile) = self.execution_profiles.get_mut(&key) {
                profile.baseline_outermost = profile.baseline_outermost.saturating_add(1);
            }
        }
        self.execution_starts
            .push((key, std::time::Instant::now(), tier, pc != 0));
    }

    fn entry_fast_grant(
        &mut self,
        id: u64,
        generation: u64,
        grant: &mut rquickjs_core::qjs::JSJitFastEntryGrant,
    ) {
        let key = runtime::FunctionKey::new(id, generation);
        if self.entry_cache_epoch == 0 {
            return;
        }
        let Some(tier) = self.fast_entry_steady(key) else {
            return;
        };
        grant.budget = FAST_ENTRY_BUDGET;
        grant.flags = if tier == runtime::Tier::Optimizing {
            rquickjs_core::qjs::JS_JIT_FAST_ENTRY_OPTIMIZED
        } else {
            0
        };
        grant.state = self.fast_entry.as_raw();
    }

    fn native_exit(&mut self, id: u64, generation: u64, pc: u32, exit_kind: u32) {
        // A granted execution skipped native_enter; its non-DONE exit still
        // reports here for deopt, retry and exception accounting.
        let unpaired = exit_kind & rquickjs_core::qjs::JS_JIT_EXIT_FAST_UNPAIRED != 0;
        let exit_kind = exit_kind & !rquickjs_core::qjs::JS_JIT_EXIT_FAST_UNPAIRED;
        self.native_exits = self.native_exits.saturating_add(1);
        let key = runtime::FunctionKey::new(id, generation);
        if exit_kind == rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_DONE {
            if let Some(observed) = self.osr_validation.take_native_return(id, generation) {
                // Native returns complete the call signature exactly like the
                // interpreter's OP_return feedback; the sentinel pc keeps them
                // apart from bytecode return sites and deopt guard pcs.
                self.feedback
                    .observe_return(key, NATIVE_RETURN_FEEDBACK_PC, observed);
            }
        }
        // An unmatched callback must not discard a different active timer.
        // Actual C invocations, including recursive and OSR entries, are LIFO.
        let mut measured_tier2_loss = false;
        if let Some((_, start, tier, osr)) = self
            .execution_starts
            .last()
            .copied()
            .filter(|(active, _, _, _)| !unpaired && *active == key)
        {
            self.execution_starts.pop();
            let elapsed = start.elapsed().as_nanos().try_into().unwrap_or(u64::MAX);
            let optimized = tier == runtime::Tier::Optimizing;
            let profile = self.execution_profiles.entry(key).or_default();
            if optimized {
                profile.record_optimized(elapsed);
                if self.native_recursive.contains(&key) {
                    // Keep the trial, but never classify a collapsed
                    // recursion tree against per-call baseline samples.
                    profile.tier2_trial_decided = true;
                }
                // Like V8/JSC tier-down decisions, use a bounded observation
                // window and a material margin instead of reacting to one
                // noisy invocation. Cross-multiply so the decision remains
                // deterministic and division-free: after at least eight
                // samples, Tier-2's fastest invocation must not be 25% slower
                // than the measured baseline average for the same immutable
                // function identity. The coordinator only acts on a loss when
                // that baseline is still installed to fall back to.
                measured_tier2_loss = classify_tier2_trial(profile) == Some(true);
                // A forced test tier must stay optimized: under sanitizer or
                // loaded hosts, timing noise alone can classify a loss.
                #[cfg(feature = "test-support")]
                if self.config.force_optimized() {
                    measured_tier2_loss = false;
                }
                if exit_kind == rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_DONE {
                    if let Some(baseline_average) =
                        profile.baseline_ns.checked_div(profile.baseline_executions)
                    {
                        let saved = baseline_average.saturating_sub(elapsed);
                        if saved != 0
                            && self.coordinator.record_benefit(
                                key,
                                runtime::Tier::Optimizing,
                                saved,
                            )
                        {
                            self.benefit_recordings = self.benefit_recordings.saturating_add(1);
                            self.measured_benefit_ns =
                                self.measured_benefit_ns.saturating_add(saved);
                        }
                    }
                }
            } else {
                profile.record_baseline(elapsed, !osr);
            }
        }
        if measured_tier2_loss && self.coordinator.demote_unprofitable_optimized(key) {
            // QuickJS may hold a cached entry handle. Force its next call to
            // reacquire after the current pinned invocation has completed.
            self.invalidate_entry_cache();
        }
        self.coordinator
            .record_side_path_entries(self.osr_validation.take_side_path_entries());
        if exit_kind == rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_RETRY_INTERPRETER {
            self.native_retries = self.native_retries.saturating_add(1);
            let validation_retry = self
                .osr_validation
                .take_validation_retry(id, generation, pc);
            if is_generated_osr_retry(pc, exit_kind, validation_retry) {
                self.osr_generated_retries = self.osr_generated_retries.saturating_add(1);
            }
        } else if exit_kind == rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT {
            // Side-exit accounting can unpublish optimizing code immediately,
            // before the next maintenance pass. Invalidate handles belonging
            // to still-active recursive frames before they can be cached again.
            self.invalidate_entry_cache();
            self.native_fallbacks = self.native_fallbacks.saturating_add(1);
            let key = runtime::FunctionKey::new(id, generation);
            if let Some((guard, observed)) = self.osr_validation.take_deopt_guard(id, generation) {
                if self
                    .coordinator
                    .record_optimized_side_exit_profile(key, guard, observed)
                    == runtime::SideExitAction::StablePathThreshold
                {
                    if let Some(snapshot) = self.tier2_sources.get(&key).cloned() {
                        let guard_pc = self
                            .coordinator
                            .pin(key, runtime::Tier::Optimizing)
                            .and_then(|pin| {
                                pin.artifact().optimized_metadata().and_then(|metadata| {
                                    metadata.deopt_sites().iter().find_map(|(_, map)| {
                                        (map.guard() == guard).then_some(map.resume_pc())
                                    })
                                })
                            });
                        if let (Some(guard_pc), Some(observed)) = (guard_pc, observed) {
                            self.feedback.observe_type(
                                key,
                                guard_pc,
                                runtime::FeedbackKind::Exit,
                                observed,
                            );
                        }
                        let feedback = self.feedback.snapshot(self.clock.max(1));
                        let profile = guard_pc.zip(observed).map(|(guard_pc, observed)| {
                            runtime::SidePathProfile::new(
                                key,
                                runtime::GuardId::new(guard),
                                guard_pc,
                                observed,
                                feedback.epoch(),
                            )
                        });
                        if profile.is_some_and(|profile| {
                            self.coordinator
                                .queue_side_path(key, snapshot, feedback, profile)
                                .is_ok()
                        }) {
                            self.cold_metrics_dirty = true;
                            self.stable_path_compile_requests =
                                self.stable_path_compile_requests.saturating_add(1);
                        }
                    }
                }
            } else {
                self.coordinator.record_deopt(true);
            }
        }
        self.maintenance_if_due(true);
    }

    fn function_retire(&mut self, id: u64, generation: u64) {
        let key = runtime::FunctionKey::new(id, generation);
        self.requested.remove(&key);
        self.native_recursive.remove(&key);
        self.native_call_plans.remove(&key);
        self.queue_reasons.remove(&key);
        self.prequeue_backoff.remove(&key);
        self.hotness.remove(&key);
        self.optimizing_requested.remove(&key);
        self.optimizing_hotness.remove(&key);
        self.optimizing_snapshots.remove(&key);
        self.tier2_deferred.remove(&key);
        self.tier2_untranslatable.remove(&key);
        self.tier2_sources.remove(&key);
        self.optimized_vocabulary.remove(&key);
        self.baseline_property_refreshed.remove(&key);
        self.entry_tiers.remove(&key);
        self.feedback.forget_function(key);
        self.coordinator.retire(key);
        self.maintenance();
    }

    fn runtime_detach(&mut self) {
        self.entry_cache_epoch = 0;
        self.fast_entry.epoch.store(0, Ordering::Relaxed);
        self.workers.shutdown(&mut self.coordinator);
        self.coordinator.shutdown();
        self.maintenance();
    }

    fn memory_used(&self) -> usize {
        self.coordinator.cache_bytes()
    }
}

/// Builder for an owning [`JitRuntime`].
#[derive(Clone, Debug, Default)]
pub struct JitRuntimeBuilder {
    config: JitConfig,
}

impl JitRuntimeBuilder {
    /// Sets the JIT policy and resource limits.
    pub fn config(mut self, config: JitConfig) -> Self {
        self.config = config;
        self
    }

    /// Constructs a QuickJS runtime with an attached JIT backend.
    pub fn build(self) -> Result<JitRuntime, JitError> {
        let runtime = Runtime::new()?;
        let jit = Jit::attach(&runtime, self.config)?;
        Ok(JitRuntime {
            jit: Some(jit),
            runtime: Some(runtime),
        })
    }

    /// Constructs a plain QuickJS interpreter without attaching a JIT backend.
    ///
    /// This is useful for differential tests and benchmarks that need an
    /// interpreter baseline without profiling, snapshot, or compiler overhead.
    pub fn build_interpreter(self) -> Result<JitRuntime, JitError> {
        let runtime = Runtime::new()?;
        Ok(JitRuntime {
            jit: None,
            runtime: Some(runtime),
        })
    }
}

/// An owning QuickJS runtime paired with its JIT guard.
pub struct JitRuntime {
    jit: Option<Jit>,
    runtime: Option<Runtime>,
}

impl JitRuntime {
    /// Starts building an owning JIT runtime.
    pub fn builder() -> JitRuntimeBuilder {
        JitRuntimeBuilder::default()
    }

    /// Returns whether this runtime owns an attached JIT backend.
    pub const fn has_jit(&self) -> bool {
        self.jit.is_some()
    }

    /// Returns the attached JIT guard.
    ///
    /// Panics when this runtime was created with
    /// [`JitRuntimeBuilder::build_interpreter`].
    pub const fn jit(&self) -> &Jit {
        match &self.jit {
            Some(jit) => jit,
            None => panic!("interpreter runtime has no JIT guard"),
        }
    }

    /// Returns runtime metrics, or disabled zero metrics for an interpreter.
    pub fn metrics(&self) -> JitMetrics {
        match &self.jit {
            Some(jit) => jit.metrics(),
            None => JitMetrics::disabled(),
        }
    }
}

impl Deref for JitRuntime {
    type Target = Runtime;

    fn deref(&self) -> &Self::Target {
        self.runtime
            .as_ref()
            .expect("QuickJS runtime is present until drop")
    }
}

fn drop_jit_before_runtime<J, R>(jit: &mut Option<J>, runtime: &mut Option<R>) {
    drop(jit.take());
    drop(runtime.take());
}

impl Drop for JitRuntime {
    fn drop(&mut self) {
        drop_jit_before_runtime(&mut self.jit, &mut self.runtime);
    }
}

#[cfg(test)]
mod jit_runtime_drop_tests {
    use super::{drop_jit_before_runtime, JitRuntime};
    use std::sync::{Arc, Mutex};

    #[test]
    fn reports_whether_a_jit_backend_is_attached() {
        let automatic = JitRuntime::builder().build().unwrap();
        let interpreter = JitRuntime::builder().build_interpreter().unwrap();

        assert!(automatic.has_jit());
        assert!(!interpreter.has_jit());
    }

    struct DropProbe {
        label: &'static str,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.events.lock().unwrap().push(self.label);
        }
    }

    #[test]
    fn owning_runtime_drops_jit_guard_before_quickjs_runtime() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut jit = Some(DropProbe {
            label: "jit",
            events: Arc::clone(&events),
        });
        let mut runtime = Some(DropProbe {
            label: "runtime",
            events: Arc::clone(&events),
        });

        drop_jit_before_runtime(&mut jit, &mut runtime);

        assert_eq!(*events.lock().unwrap(), ["jit", "runtime"]);
    }
}

#[cfg(all(
    test,
    feature = "compiler",
    any(
        all(
            target_os = "macos",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "windows",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "linux",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )
))]
mod production_environment_tests {
    use super::*;

    // Count allocations on this test thread only; background compilers and
    // concurrently running tests keep using the system allocator unobserved.
    struct CountingAllocator;
    std::thread_local! {
        static ALLOCATIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }
    #[global_allocator]
    static TEST_ALLOCATOR: CountingAllocator = CountingAllocator;
    unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|count| {
                if let Some(n) = count.get() {
                    count.set(Some(n + 1));
                }
            });
            unsafe { std::alloc::GlobalAlloc::alloc(&std::alloc::System, layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
            unsafe { std::alloc::GlobalAlloc::dealloc(&std::alloc::System, ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|count| {
                if let Some(n) = count.get() {
                    count.set(Some(n + 1));
                }
            });
            unsafe { std::alloc::GlobalAlloc::realloc(&std::alloc::System, ptr, layout, size) }
        }
    }

    fn call_event(types: &[u32], callsite: bool) -> rquickjs_core::qjs::JSJitFeedbackEvent {
        use rquickjs_core::qjs;
        // All-zero scalar fields and a null pointer are valid for this C event.
        let mut event: qjs::JSJitFeedbackEvent = unsafe { core::mem::zeroed() };
        event.struct_size = core::mem::size_of_val(&event) as u32;
        event.kind = qjs::JSJitFeedbackKind_JS_JIT_FEEDBACK_CALL;
        event.function.id = 7;
        event.function.generation = 3;
        event.callee.id = 8;
        event.callee.generation = 4;
        event.pc = 12;
        event.types = types.as_ptr();
        event.type_count = types.len() as u32;
        if callsite {
            event.flags = qjs::JS_JIT_FEEDBACK_CALL_SITE;
        }
        event
    }

    #[test]
    fn native_exit_publishes_exact_metrics_during_nested_calls_and_cold_updates() {
        use rquickjs_core::{qjs, runtime::JitBackend};
        let metrics = Arc::new(Mutex::new(JitMetrics::default()));
        let mut backend = ProductionBackend::new(
            1,
            &abi::AbiInfo::linked().unwrap(),
            JitConfig::default(),
            Arc::clone(&metrics),
        )
        .unwrap();
        backend.poll();
        backend.native_enter(7, 3, 0);
        backend.native_enter(8, 4, 0);
        backend.native_exit(8, 4, 0, qjs::JSJitExitKind_JS_JIT_EXIT_DONE);
        {
            let published = metrics.lock().unwrap();
            assert_eq!((published.native_entries, published.native_exits), (2, 1));
        }

        // A hot event can request a snapshot after its own maintenance check.
        // The next native exit must publish that cold counter immediately.
        let mut event: qjs::JSJitHotEvent = unsafe { core::mem::zeroed() };
        event.struct_size = core::mem::size_of_val(&event) as u32;
        event.function.id = 9;
        event.function.generation = 1;
        event.kind = qjs::JSJitHotKind_JS_JIT_HOT_CALL;
        event.count = backend.config.call_threshold();
        assert_eq!(backend.record_hot(&event), 1);
        backend
            .compiler_measurements
            .elapsed_ns
            .store(1234, Ordering::Relaxed);
        backend
            .osr_validation
            .deopt_materializations
            .store(2, Ordering::Relaxed);
        backend.native_exit(7, 3, 0, qjs::JSJitExitKind_JS_JIT_EXIT_DONE);
        let before_deopt = metrics.lock().unwrap().clone();
        assert_eq!(
            (before_deopt.native_entries, before_deopt.native_exits),
            (2, 2)
        );
        assert_eq!(before_deopt.snapshot_requests, 1);
        assert_eq!(before_deopt.compile_ns, 1234);
        assert_eq!(before_deopt.deopt_materializations, 2);

        // Deopt dirties the coordinator snapshot, which replaces even the
        // backend-owned counters. Their published values must be restored.
        backend.native_enter(8, 4, 0);
        backend.native_exit(8, 4, 0, qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT);
        let after_deopt = metrics.lock().unwrap().clone();
        assert_eq!(after_deopt.snapshot_requests, 1);
        assert_eq!(
            (after_deopt.native_entries, after_deopt.native_exits),
            (3, 3)
        );
        assert_eq!(after_deopt.native_fallbacks, 1);
        assert_eq!(after_deopt.deopts, 1);
        backend.runtime_detach();
    }

    #[test]
    fn sequential_native_entries_reuse_timing_storage_across_functions() {
        use rquickjs_core::{qjs, runtime::JitBackend};
        let mut backend = ProductionBackend::new(
            1,
            &abi::AbiInfo::linked().unwrap(),
            JitConfig::default(),
            Arc::new(Mutex::new(JitMetrics::default())),
        )
        .unwrap();
        backend.native_enter(1, 1, 0);
        backend.native_exit(1, 1, 0, qjs::JSJitExitKind_JS_JIT_EXIT_DONE);
        let mut allocations = 0;
        for id in 2..102 {
            ALLOCATIONS.with(|count| count.set(Some(0)));
            backend.native_enter(id, 1, 0);
            allocations += ALLOCATIONS.with(|count| count.replace(None).unwrap());
            // Profile creation on exit is separate from storing an active timer.
            backend.native_exit(id, 1, 0, qjs::JSJitExitKind_JS_JIT_EXIT_DONE);
        }
        assert_eq!(
            allocations, 0,
            "sequential entries must reuse warmed timing storage"
        );
        assert_eq!((backend.native_entries, backend.native_exits), (101, 101));
        backend.runtime_detach();
    }

    #[test]
    fn native_timing_preserves_recursive_tiers_across_retirement_and_exit_kinds() {
        use rquickjs_core::{qjs, runtime::JitBackend};
        use runtime::{FunctionKey, Tier};
        let mut backend = ProductionBackend::new(
            1,
            &abi::AbiInfo::linked().unwrap(),
            JitConfig::default(),
            Arc::new(Mutex::new(JitMetrics::default())),
        )
        .unwrap();
        let outer = FunctionKey::new(7, 3);
        let inner = FunctionKey::new(8, 4);
        backend.entry_tiers.insert(outer, Tier::Baseline);
        backend.native_enter(7, 3, 0);
        backend.entry_tiers.insert(inner, Tier::Optimizing);
        backend.native_enter(8, 4, 12);
        backend.entry_tiers.insert(outer, Tier::Optimizing);
        backend.native_enter(7, 3, 0);
        backend.function_retire(7, 3);
        backend.native_exit(7, 3, 0, qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT);
        backend.native_exit(8, 4, 12, qjs::JSJitExitKind_JS_JIT_EXIT_RETRY_INTERPRETER);
        backend.native_exit(7, 3, 0, qjs::JSJitExitKind_JS_JIT_EXIT_DONE);
        let a = backend.execution_profiles[&outer];
        let b = backend.execution_profiles[&inner];
        assert_eq!((a.baseline_executions, a.optimized_executions), (1, 1));
        assert_eq!((b.baseline_executions, b.optimized_executions), (0, 1));
        assert_eq!(
            (
                backend.native_entries,
                backend.native_exits,
                backend.osr_attempts
            ),
            (3, 3, 1)
        );
        backend.runtime_detach();
    }

    #[test]
    fn unmatched_native_exit_does_not_consume_another_active_timer() {
        use rquickjs_core::{qjs, runtime::JitBackend};
        use runtime::FunctionKey;
        let mut backend = ProductionBackend::new(
            1,
            &abi::AbiInfo::linked().unwrap(),
            JitConfig::default(),
            Arc::new(Mutex::new(JitMetrics::default())),
        )
        .unwrap();
        backend.native_exit(99, 1, 0, qjs::JSJitExitKind_JS_JIT_EXIT_DONE);
        backend.native_enter(7, 3, 0);
        backend.native_enter(8, 4, 0);
        // Defensive behavior for invalid callback order; C's real pairs are LIFO.
        backend.native_exit(7, 3, 0, qjs::JSJitExitKind_JS_JIT_EXIT_DONE);
        assert!(!backend
            .execution_profiles
            .contains_key(&FunctionKey::new(7, 3)));
        backend.native_exit(8, 4, 0, qjs::JSJitExitKind_JS_JIT_EXIT_EXCEPTION);
        backend.native_exit(7, 3, 0, qjs::JSJitExitKind_JS_JIT_EXIT_DONE);
        assert_eq!(
            backend.execution_profiles[&FunctionKey::new(7, 3)].baseline_executions,
            1
        );
        assert_eq!(
            backend.execution_profiles[&FunctionKey::new(8, 4)].baseline_executions,
            1
        );
        assert!(!backend
            .execution_profiles
            .contains_key(&FunctionKey::new(99, 1)));
        backend.runtime_detach();
    }

    #[test]
    fn cold_refresh_candidates_do_not_clone_unrelated_call_feedback() {
        use rquickjs_core::{qjs, runtime::JitBackend};
        let mut backend = ProductionBackend::new(
            1,
            &abi::AbiInfo::linked().unwrap(),
            JitConfig::default(),
            Arc::new(Mutex::new(JitMetrics::default())),
        )
        .unwrap();
        let snapshot = bytecode::CompileSnapshot::from_untrusted_bytecode(
            vec![qjs::QJS_JIT_OP_OBJECT, bytecode::opcode::RETURN],
            0,
            0,
            0,
            0,
        )
        .verify(bytecode::VerifyLimits::default())
        .unwrap();
        assert!(!ir::optimized_vocabulary_admits(&snapshot));
        assert!(!settles_untranslatable_candidate(&snapshot));
        for id in 10..20 {
            backend
                .optimizing_snapshots
                .insert(runtime::FunctionKey::new(id, 1), snapshot.clone());
        }
        let types = [qjs::JSJitValueType_JS_JIT_VALUE_INT32; 4];
        let mut event = call_event(&types, true);
        for pc in 0..10 {
            event.pc = pc;
            backend.record_feedback(&event);
        }
        backend.maintenance();
        let mut allocations = Vec::new();
        for end in [20, 200] {
            for pc in 10..end {
                event.pc = pc;
                backend.record_feedback(&event);
            }
            ALLOCATIONS.with(|count| count.set(Some(0)));
            backend.maintenance();
            allocations.push(ALLOCATIONS.with(|count| count.replace(None).unwrap()));
        }
        eprintln!("refresh allocations with 20/200 unrelated signatures: {allocations:?}");
        assert_eq!(
            allocations[0], allocations[1],
            "unrelated signatures must not add refresh allocations"
        );
        backend.runtime_detach();
    }

    #[test]
    fn stable_call_feedback_does_not_allocate_after_warmup() {
        use rquickjs_core::{qjs, runtime::JitBackend};
        let mut backend = ProductionBackend::new(
            1,
            &abi::AbiInfo::linked().unwrap(),
            JitConfig::default(),
            Arc::new(Mutex::new(JitMetrics::default())),
        )
        .unwrap();
        let types = [qjs::JSJitValueType_JS_JIT_VALUE_INT32; 32];
        for callsite in [false, true] {
            let event = call_event(&types, callsite);
            backend.record_feedback(&event);
            ALLOCATIONS.with(|count| count.set(Some(0)));
            for _ in 0..100 {
                backend.record_feedback(&event);
            }
            let allocations = ALLOCATIONS.with(|count| count.replace(None).unwrap());
            assert_eq!(allocations, 0, "stable CALL event must reuse storage");
        }
        backend.runtime_detach();
    }

    #[test]
    fn call_feedback_preserves_wide_arguments_and_rejects_entire_invalid_events() {
        use rquickjs_core::{qjs, runtime::JitBackend};
        use runtime::{FunctionKey, ObservedType};
        let mut backend = ProductionBackend::new(
            1,
            &abi::AbiInfo::linked().unwrap(),
            JitConfig::default(),
            Arc::new(Mutex::new(JitMetrics::default())),
        )
        .unwrap();
        let key = FunctionKey::new(7, 3);
        let mut types = vec![qjs::JSJitValueType_JS_JIT_VALUE_INT32; 32];
        types[0] = qjs::JSJitValueType_JS_JIT_VALUE_BOOL;
        types[31] = qjs::JSJitValueType_JS_JIT_VALUE_STRING;
        backend.record_feedback(&call_event(&types, false));
        backend.record_feedback(&call_event(&types, true));
        let snapshot = backend.feedback.snapshot(1);
        let call = snapshot.call_at(key).unwrap();
        assert_eq!(call.argc(), 32);
        assert_eq!(call.argument(0), [ObservedType::Bool]);
        assert_eq!(call.argument(30), [ObservedType::Int32]);
        assert_eq!(call.argument(31), [ObservedType::String]);
        let site = snapshot.call_signature_at(key, 12).unwrap();
        assert_eq!(site.targets(), [FunctionKey::new(8, 4)]);
        assert_eq!(site.argument(0), [ObservedType::Bool]);
        assert_eq!(site.argument(30), [ObservedType::Int32]);
        assert!(site.argument(31).is_empty());
        assert_eq!(site.results(), [ObservedType::String]);
        types[0] = qjs::JSJitValueType_JS_JIT_VALUE_OBJECT;
        types[31] = u32::MAX;
        for callsite in [false, true] {
            backend.record_feedback(&call_event(&types, callsite));
            assert_eq!(backend.feedback.snapshot(1), snapshot);
        }
        // A subsequent short event cannot inherit any stale scratch slots.
        backend.record_feedback(&call_event(&[qjs::JSJitValueType_JS_JIT_VALUE_BOOL], true));
        assert_eq!(
            backend
                .feedback
                .snapshot(1)
                .call_signature_at(key, 12)
                .unwrap()
                .results(),
            [ObservedType::String, ObservedType::Bool]
        );
        backend.runtime_detach();
    }

    #[test]
    fn deopt_demotions_invalidate_entry_cache_before_periodic_maintenance() {
        use rquickjs_core::runtime::JitBackend;
        let mut backend = ProductionBackend::new(
            1,
            &abi::AbiInfo::linked().unwrap(),
            JitConfig::default(),
            Arc::new(Mutex::new(JitMetrics::default())),
        )
        .unwrap();
        backend.maintenance();
        let epoch = backend.entry_cache_epoch();
        assert_ne!(epoch, 0);
        for guard in [1, 2] {
            backend.osr_validation.mark_deopt_guard(7, 3, guard, None);
            backend.native_exit(7, 3, 0, rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT);
        }
        assert_eq!(backend.coordinator.metrics().optimized_demotions, 1);
        assert_eq!(backend.hot_ticks_since_maintenance, 2);
        assert_ne!(backend.entry_cache_epoch(), epoch);
        backend.runtime_detach();
    }

    #[test]
    fn artifact_environment_uses_runtime_abi_target_features_and_config() {
        let first = Runtime::new().unwrap();
        let second = Runtime::new().unwrap();
        let info = abi::AbiInfo::linked().unwrap();
        let compiler = compiler::baseline::BaselineCompiler::host();
        let base = artifact_environment(
            first.jit_runtime_id(),
            &info,
            &JitConfig::default(),
            &compiler,
        );
        let other_runtime = artifact_environment(
            second.jit_runtime_id(),
            &info,
            &JitConfig::default(),
            &compiler,
        );
        let other_config = artifact_environment(
            first.jit_runtime_id(),
            &info,
            &JitConfig::builder().call_threshold(99).build().unwrap(),
            &compiler,
        );
        assert_ne!(base.runtime_id, other_runtime.runtime_id);
        assert_ne!(base.runtime_id, 0);
        assert_ne!(base.target_isa, 0);
        assert_eq!(
            base.target_isa,
            compiler.target_identity().triple_fingerprint()
        );
        assert_eq!(
            base.cpu_features,
            compiler.target_identity().codegen_fingerprint()
        );
        assert_ne!(base.abi_fingerprint, 0);
        assert_ne!(base.config_fingerprint, other_config.config_fingerprint);
        assert_ne!(base.cpu_features, 0);
    }
}
