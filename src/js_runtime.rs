//! V8-based JavaScript runtime for executing 17track's sign generation module.
//!
//! Uses `deno_core` to embed a V8 engine that can run the obfuscated fingerprint
//! JS module, with mocked browser globals (navigator, screen, document, canvas).
//!
//! The sign module (chunk 839 / ff19fa74) contains an embedded WASM binary using
//! wasm-bindgen. The module's JS wrapper has a stale Uint8Array cache issue with
//! WASM memory views, so we bypass it and call the raw WASM exports directly,
//! reading the result string from WASM linear memory ourselves.

use anyhow::Result;
use deno_core::{JsRuntime, PollEventLoopOptions, RuntimeOptions};

/// Browser mocks script that provides fake DOM/browser globals.
///
/// The sign module probes various browser APIs during fingerprint generation.
/// We provide deterministic mock values that produce a valid sign.
///
/// Embedded at compile time from `js_runtime/browser_mocks.js`.
const BROWSER_MOCKS: &str = include_str!("js_runtime/browser_mocks.js");

/// Webpack interception script that captures the module factory from chunk 839.
///
/// The chunk registers itself via:
/// ```js
/// (self["webpackChunk_N_E"] = self["webpackChunk_N_E"] || []).push([[839], {4279: factory}])
/// ```
/// We intercept the `push()` call to capture the factory and execute it.
///
/// Embedded at compile time from `js_runtime/webpack_intercept.js`.
const WEBPACK_INTERCEPT: &str = include_str!("js_runtime/webpack_intercept.js");

/// Sign generator that uses V8 to execute 17track's fingerprint JS module.
pub struct SignGenerator {
    runtime: JsRuntime,
    initialized: bool,
}

impl SignGenerator {
    /// Create a new V8 runtime with browser mocks.
    pub fn new() -> Result<Self> {
        let runtime = JsRuntime::new(RuntimeOptions::default());

        let mut generator = Self {
            runtime,
            initialized: false,
        };

        // Install browser mocks
        generator
            .runtime
            .execute_script("[browser_mocks]", BROWSER_MOCKS)
            .map_err(|e| anyhow::anyhow!("Failed to install browser mocks: {}", e))?;

        // Install webpack interception
        generator
            .runtime
            .execute_script("[webpack_intercept]", WEBPACK_INTERCEPT)
            .map_err(|e| anyhow::anyhow!("Failed to install webpack intercept: {}", e))?;

        Ok(generator)
    }

    /// Initialize with the sign module JS content.
    ///
    /// Executes the ff19fa74 chunk JS which registers its module factory,
    /// then extracts and initializes the module (including WASM compilation).
    pub async fn initialize(&mut self, sign_module_js: &str) -> Result<()> {
        // Execute the chunk JS - triggers webpackChunk_N_E.push() interception
        self.runtime
            .execute_script("[sign_module]", sign_module_js.to_string())
            .map_err(|e| anyhow::anyhow!("Failed to execute sign module: {}", e))?;

        // Run event loop to handle any async initialization
        self.runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await
            .map_err(|e| anyhow::anyhow!("Event loop error during module load: {}", e))?;

        // Find and execute the module, then call default() to initialize WASM
        let init_script = r#"
            (async function() {
                var moduleExports = null;
                var targetIds = ["4279"];

                for (var i = 0; i < targetIds.length; i++) {
                    if (__captured_modules[targetIds[i]]) {
                        moduleExports = __executeModule(targetIds[i]);
                        break;
                    }
                }

                // Fallback: search all captured modules for get_fingerprint
                if (!moduleExports) {
                    for (var id in __captured_modules) {
                        try {
                            var exports = __executeModule(id);
                            if (exports && exports.get_fingerprint) {
                                moduleExports = exports;
                                break;
                            }
                        } catch(e) {}
                    }
                }

                if (!moduleExports) {
                    throw new Error("Could not find sign module. Captured: " + Object.keys(__captured_modules).join(", "));
                }

                globalThis.__signModule = moduleExports;

                // Call default() to initialize (compiles WASM, sets up exports)
                if (typeof moduleExports.default === "function") {
                    await moduleExports.default();
                }

                // Save references to raw WASM exports for direct memory access.
                // The JS wrapper's string decode uses a cached Uint8Array that becomes
                // stale after WASM memory growth, returning all-zero strings. We bypass
                // this by reading WASM memory directly with fresh views.
                if (globalThis.__wasmInstance) {
                    var exp = globalThis.__wasmInstance.exports;
                    globalThis.__rawWasm = {
                        get_fingerprint: exp.get_fingerprint,
                        stack: exp.__wbindgen_add_to_stack_pointer,
                        memory: exp.memory,
                        free: exp.__wbindgen_export_2  // __wbindgen_free
                    };
                }

                return "ok";
            })()
        "#;

        let result = self
            .runtime
            .execute_script("[init_sign_module]", init_script)
            .map_err(|e| anyhow::anyhow!("Failed to init sign module: {}", e))?;

        let resolved = self.runtime.resolve(result);
        self.runtime
            .with_event_loop_promise(resolved, PollEventLoopOptions::default())
            .await
            .map_err(|e| anyhow::anyhow!("Sign module init failed: {}", e))?;

        self.initialized = true;
        Ok(())
    }

    /// Generate a sign value by calling the WASM get_fingerprint export directly.
    ///
    /// Bypasses the JS wrapper's broken string decode by reading the result
    /// string from WASM linear memory with fresh Uint8Array/Int32Array views.
    pub async fn generate_sign(&mut self) -> Result<String> {
        if !self.initialized {
            anyhow::bail!("SignGenerator not initialized - call initialize() first");
        }

        let gen_script = r#"
            (function() {
                var rw = globalThis.__rawWasm;
                if (!rw || !rw.get_fingerprint || !rw.stack || !rw.memory) {
                    throw new Error("Raw WASM exports not available");
                }

                // Allocate return pointer on the WASM stack
                var retptr = rw.stack(-16);
                try {
                    // Call get_fingerprint(retptr, mousePointsPtr=0, mousePointsLen=0)
                    rw.get_fingerprint(retptr, 0, 0);

                    // Read ptr+len from retptr using FRESH Int32Array view
                    // (avoids stale buffer reference after WASM memory growth)
                    var i32 = new Int32Array(rw.memory.buffer);
                    var ptr = i32[retptr / 4 + 0];
                    var len = i32[retptr / 4 + 1];

                    if (len <= 0 || len > 100000) {
                        throw new Error("Invalid sign length: " + len + " (ptr=" + ptr + ")");
                    }

                    // Decode UTF-8 string from WASM memory with FRESH Uint8Array view
                    var u8 = new Uint8Array(rw.memory.buffer);
                    var bytes = u8.slice(ptr, ptr + len);
                    var sign = new TextDecoder("utf-8").decode(bytes);

                    // Free the WASM-allocated string
                    if (rw.free) {
                        try { rw.free(ptr, len, 1); } catch(e) {}
                    }

                    globalThis.__signResult = sign;
                    return "ok";
                } finally {
                    rw.stack(16); // restore stack pointer
                }
            })()
        "#;

        self.runtime
            .execute_script("[generate_sign]", gen_script)
            .map_err(|e| anyhow::anyhow!("Failed to call get_fingerprint: {}", e))?;

        self.runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await
            .ok();

        // Read the sign result
        let read_script = r#"
            (function() {
                var result = globalThis.__signResult;
                if (result === undefined || result === null) {
                    return JSON.stringify({"error": "Sign generation returned no result"});
                }
                return JSON.stringify({"sign": result});
            })()
        "#;

        let result = self
            .runtime
            .execute_script("[read_sign]", read_script)
            .map_err(|e| anyhow::anyhow!("Failed to read sign result: {}", e))?;

        let json_str: String = {
            let context = self.runtime.main_context();
            let isolate = self.runtime.v8_isolate();
            let mut handle_scope = deno_core::v8::HandleScope::new(isolate);
            let handle_scope = unsafe { std::pin::Pin::new_unchecked(&mut handle_scope) };
            let handle_scope = &mut handle_scope.init();
            let context_local = deno_core::v8::Local::new(handle_scope, context);
            let scope = &mut deno_core::v8::ContextScope::new(handle_scope, context_local);
            let local = deno_core::v8::Local::new(scope, &result);
            let str_val = local
                .to_string(scope)
                .ok_or_else(|| anyhow::anyhow!("V8 result is not a string"))?;
            str_val.to_rust_string_lossy(scope)
        };

        let parsed: serde_json::Value = serde_json::from_str(&json_str).map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse sign result JSON: {} (raw: {})",
                e,
                json_str
            )
        })?;

        if let Some(error) = parsed.get("error").and_then(|v| v.as_str()) {
            anyhow::bail!("Sign generation error: {}", error);
        }

        parsed
            .get("sign")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Sign not found in result: {}", json_str))
    }

    /// Check if the runtime has been initialized with the sign module.
    ///
    /// Returns `true` if `initialize()` has been called successfully and the
    /// V8 runtime is ready to generate signs.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}
