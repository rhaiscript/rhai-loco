#![doc = include_str!("../README.md")]

use ::serde::{de::DeserializeOwned, Deserialize, Serialize};
use axum::{extract::FromRequestParts, http::request::Parts, Extension, Router as AxumRouter};
use loco_rs::app::{AppContext, Initializer};
use loco_rs::prelude::*;
use serde_json::Value;
use std::{
    collections::HashMap,
    fmt::Debug,
    fs::read_dir,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
};
use tracing::{debug, info, trace, trace_span};

// Re-export useful Rhai types and functions.
use rhai::module_resolvers::FileModuleResolver;
pub use rhai::serde::{from_dynamic, to_dynamic};
pub use rhai::*;
pub use tera;

/// Type alias for `Result<T, Box<EvalAltResult>>`.
pub type RhaiResult<T> = std::result::Result<T, Box<EvalAltResult>>;

/// Target namespace path for logging.
///
/// Notice that this is changed to start with the `loco_rs` crate in order for script logs to be
/// visible under Loco.
pub const ROOT: &str = "loco_rs::scripting::rhai_script";

/// Directory containing Rhai scripts.
pub const SCRIPTS_DIR: &'static str = "assets/scripts";

/// Directory containing Rhai scripts for Tera filters.
pub const FILTER_SCRIPTS_DIR: &'static str = "assets/scripts/tera/filters";

/// Global Rhai [`Engine`] instance for scripts evaluation.
pub static ENGINE: OnceLock<Engine> = OnceLock::new();

/// Global Rhai [`Engine`] instance for filter scripts evaluation.
pub static FILTERS_ENGINE: OnceLock<Engine> = OnceLock::new();

/// Global `RhaiScript` instance for scripts evaluation.
pub static RHAI_SCRIPT: OnceLock<RhaiScript> = OnceLock::new();

/// Error message for script file not found.
const SCRIPT_FILE_NOT_FOUND: &str = "script file not found";

/// Type that wraps a scripting engine for use in [`Axum`][axum] handlers.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ScriptingEngine<E>(pub E);

impl<E> ScriptingEngine<E> {
    /// Creates a new [`ScriptingEngine`] that wraps the given scripting engine
    #[inline(always)]
    #[must_use]
    pub fn new(engine: E) -> Self {
        Self(engine)
    }
}

impl<E> From<E> for ScriptingEngine<E> {
    fn from(inner: E) -> Self {
        Self::new(inner)
    }
}

impl<S, E> FromRequestParts<S> for ScriptingEngine<E>
where
    S: Send + Sync,
    E: Clone + Send + Sync + 'static,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        let Extension(tl): Extension<Self> = Extension::from_request_parts(parts, state)
            .await
            .expect("Scripting layer missing. Is it installed?");

        Ok(tl)
    }
}

/// A scripting engine based on [`Rhai`](https://rhai.rs).
#[derive(Debug, Clone)]
pub struct RhaiScript {
    /// Path to the directory containing Rhai scripts.
    scripts_path: Arc<PathBuf>,
    /// Cache of compiled Rhai scripts in [`AST`] form.
    cache: Arc<RwLock<HashMap<PathBuf, Arc<AST>>>>,
}

impl RhaiScript {
    /// File extension for Rhai scripts.
    pub const SCRIPTS_EXT: &'static str = "rhai";

    /// Get a new [`RhaiScript`] instance.
    ///
    /// The methods [`new`][`RhaiScript::new`] or [`new_with_setup`][`RhaiScript::new_with_setup`] must be called first.
    ///
    /// # Panics
    ///
    /// Panics if called before [`new`][`RhaiScript::new`] or [`new_with_setup`][`RhaiScript::new_with_setup`].
    #[inline(always)]
    pub fn get_instance() -> Self {
        RHAI_SCRIPT.get().unwrap().clone()
    }

    /// Create a new [`RhaiScript`] instance.
    ///
    /// This method can only be called once. A Rhai [`Engine`] instance is created and shared globally.
    ///
    /// # Panics
    ///
    /// Panics if called more than once.
    ///
    /// # Errors
    ///
    /// Error if the scripts directory does not exist.
    #[inline(always)]
    pub fn new(scripts_path: impl Into<PathBuf>) -> Result<Self> {
        Self::new_with_setup(scripts_path, |_| {})
    }

    /// Create a new [`RhaiScript`] instance with custom setup.
    ///
    /// This method can only be called once. A Rhai [`Engine`] instance is created and shared globally.
    ///
    /// # Panics
    ///
    /// Panics if called more than once.
    ///
    /// # Errors
    ///
    /// Error if the scripts directory does not exist.
    pub fn new_with_setup(
        scripts_path: impl Into<PathBuf>,
        setup: impl FnOnce(&mut Engine),
    ) -> Result<Self> {
        let scripts_path = scripts_path.into();

        if !scripts_path.exists() {
            return Err(Error::string(&format!(
                "missing scripts directory: `{}`",
                scripts_path.to_string_lossy()
            )));
        }

        let mut engine = Engine::new();

        let mut resolver = FileModuleResolver::new_with_path(SCRIPTS_DIR);
        resolver.enable_cache(false);

        engine
            .set_module_resolver(resolver)
            .on_print(|message| info!(target: ROOT, message))
            .on_debug(
                |message, source, pos| debug!(target: ROOT, ?message, source, position = ?pos),
            );

        setup(&mut engine);

        ENGINE
            .set(engine)
            .expect("`RhaiScript::new` or `RhaiScript::new_with_setup` can be called only once.");

        RHAI_SCRIPT
            .set(Self {
                scripts_path: Arc::new(scripts_path),
                cache: Arc::new(RwLock::new(HashMap::new())),
            })
            .unwrap();

        Ok(Self::get_instance())
    }

    /// Get a reference to the Rhai [`Engine`].
    #[inline(always)]
    #[must_use]
    pub fn engine(&self) -> &Engine {
        ENGINE.get().unwrap()
    }

    /// Convert a [Rhai error][EvalAltResult] to a [Loco error][Result].
    ///
    /// If the error is a [runtime error][EvalAltResult::ErrorRuntime],
    /// it is converted using the provided closure.
    ///
    /// Otherwise, the error is converted via [`Error::msg`].
    pub fn convert_runtime_error<T>(
        &self,
        err: Box<EvalAltResult>,
        converter: impl FnOnce(String) -> Result<T>,
    ) -> Result<T> {
        match *err {
            EvalAltResult::ErrorRuntime(r, _) => converter(r.to_string()),
            e => Err(Error::msg(e)),
        }
    }

    /// Run a script if it exists.
    ///
    /// Return `Value::Null` if the script does not exist.
    ///
    /// # Errors
    ///
    /// * Error if there is a syntax error during compilation.
    /// * Error if there is an error during script evaluation.
    #[inline(always)]
    pub fn run_script_if_exists(
        &self,
        script_file: &str,
        data: &mut (impl Serialize + DeserializeOwned + Debug),
        fn_name: &str,
        args: impl FuncArgs,
    ) -> RhaiResult<Value> {
        self.run_script(script_file, data, fn_name, args)
            .or_else(|err| match *err {
                EvalAltResult::ErrorSystem(s, e)
                    if s == SCRIPT_FILE_NOT_FOUND && e.to_string() == script_file =>
                {
                    Ok(Value::Null)
                }
                _ => Err(err),
            })
    }

    /// Run a script.
    ///
    /// # Errors
    ///
    /// * Error if the script file does not exist.
    /// * Error if there is a syntax error during compilation.
    /// * Error if there is an error during script evaluation.
    pub fn run_script(
        &self,
        script_file: &str,
        data: &mut (impl Serialize + DeserializeOwned + Debug),
        fn_name: &str,
        args: impl FuncArgs,
    ) -> RhaiResult<Value> {
        let mut script_path = self.scripts_path.join(script_file);

        if script_path.extension().is_none() {
            script_path.set_extension(Self::SCRIPTS_EXT);
        }

        let _ = trace_span!("run_script").enter();

        if !script_path.exists() {
            debug!(target: ROOT, script = script_path.to_string_lossy().as_ref(), message = SCRIPT_FILE_NOT_FOUND);
            return Err(EvalAltResult::ErrorSystem(
                SCRIPT_FILE_NOT_FOUND.to_string(),
                script_file.into(),
            )
            .into());
        }

        let mut cache = self.cache.write().unwrap();

        let ast = if let Some(ast) = cache.get(&script_path) {
            ast
        } else {
            let mut ast = self.engine().compile_file(script_path.clone())?;
            ast.set_source(script_path.to_string_lossy().as_ref());
            cache
                .entry(script_path)
                .or_insert_with(|| Arc::new(ast.clone()))
        };

        let source = ast.source();
        debug!(fn_name, ?data, source, "Rhai: call function");

        let mut obj = to_dynamic(&*data).unwrap();
        let options = CallFnOptions::new().bind_this_ptr(&mut obj);

        let result = self
            .engine()
            .call_fn_with_options(options, &mut Scope::new(), ast, fn_name, args)
            .map(|v| from_dynamic(&v).unwrap())
            .map_err(|err| match *err {
                EvalAltResult::ErrorInFunctionCall(f, _, e, Position::NONE) if f == fn_name => e,
                _ => err,
            });

        *data = from_dynamic(&obj).unwrap();

        debug!(?result, ?data, fn_name, source, "Rhai: function returns");

        result
    }

    /// Register Tera filters from Rhai scripts on a raw [`Tera`](tera::Tera) instance.
    ///
    /// If the Tera i18n function `t` is provided, it is also registered into the Rhai [`Engine`]
    /// for use in filter scripts.
    ///
    /// # Errors
    ///
    /// * Error if the filter scripts directory does not exist.
    /// * Error if there is a syntax error in any script during compilation.
    pub fn register_tera_filters(
        tera: &mut tera::Tera,
        scripts_path: impl AsRef<Path>,
        engine_setup: impl FnOnce(&mut Engine),
        i18n: Option<impl tera::Function + 'static>,
    ) -> Result<()> {
        let path = scripts_path.as_ref();

        if !path.exists() {
            return Err(Error::string(&format!(
                "missing scripts directory: `{}`",
                path.to_string_lossy()
            )));
        }

        let span = trace_span!("register_filters", dir = ?path);
        let _ = span.enter();

        let engine = FILTERS_ENGINE.get_or_init(|| {
            let mut engine = Engine::new();

            engine_setup(&mut engine);

            engine
                .on_print(|message| info!(target: ROOT, message))
                .on_debug(
                    |message, source, pos| debug!(target: ROOT, ?message, source, position = ?pos),
                );

            if let Some(i18n) = i18n {
                let i18n = Arc::new(i18n);

                let t = i18n.clone();
                engine.register_fn("t", move |args: Map| -> RhaiResult<Dynamic> {
                    let map: HashMap<String, Value> = args
                        .into_iter()
                        .map(|(k, v)| -> RhaiResult<(String, Value)> {
                            Ok((k.to_string(), from_dynamic(&v)?))
                        })
                        .collect::<RhaiResult<_>>()?;
                    match t.call(&map) {
                        Ok(v) => Ok(to_dynamic(v)?),
                        Err(e) => Err(e.to_string().into()),
                    }
                });

                let t = i18n.clone();
                engine.register_fn("t", move |key: &str, lang: &str| -> RhaiResult<Dynamic> {
                    let mut map = HashMap::new();
                    let _ = map.insert("key".to_string(), key.into());
                    let _ = map.insert("lang".to_string(), lang.into());
                    match t.call(&map) {
                        Ok(v) => Ok(to_dynamic(v)?),
                        Err(e) => Err(e.to_string().into()),
                    }
                });

                info!(target: ROOT, "i18n function loaded into Rhai engine");
            }

            engine
        });

        for entry in read_dir(path)? {
            let entry = entry?;
            let script = entry.path();

            if script.is_dir() {
                debug!(target: ROOT, dir = ?entry.file_name().to_string_lossy(), "skip dir");
                continue;
            } else if script
                .extension()
                .map_or(true, |ext| ext.to_string_lossy() != Self::SCRIPTS_EXT)
            {
                debug!(target: ROOT, file = ?entry.file_name().to_string_lossy(), "skip non-script file");
                continue;
            }

            let mut ast = engine.compile_file(script.clone()).map_err(|err| {
                Error::string(&(format!("`{}`: {err}", entry.file_name().to_string_lossy())))
            })?;
            ast.set_source(script.to_string_lossy().as_ref());
            let shared_ast = Arc::new(ast);
            debug!(target: ROOT, file = ?entry.file_name().to_string_lossy(), "compile script");

            shared_ast.iter_functions()
                .filter(|fn_def| fn_def.access != FnAccess::Private && fn_def.params.len() == 1)
                .for_each(|fn_def| {
                    let fn_name = fn_def.name.to_string();
                    let ast = shared_ast.clone();

                    let f = move |value: &Value,
                                  variables: &HashMap<String, Value>|
                          -> tera::Result<Value> {
                        trace!(target: ROOT, fn_name, ?value, ?variables, "Rhai: call Tera filter");

                        let mut obj = to_dynamic(value).unwrap();
                        let dict = to_dynamic(variables).unwrap().cast::<Map>();

                        let scope = &mut Scope::new();
                        dict.iter().for_each(|(k, v)| {
                            scope.push_dynamic(k.clone(), v.clone());
                        });

                        let options = CallFnOptions::new().bind_this_ptr(&mut obj);
                        let value = engine
                            .call_fn_with_options::<Dynamic>(options, scope, &ast, &fn_name, (dict,))
                            .map_err(tera::Error::msg)?;

                        let value = from_dynamic(&value).unwrap();
                        trace!(target: ROOT, ?value, fn_name, ?variables, "Rhai: return value from Tera filter");

                        Ok(value)
                    };

                    tera.register_filter(fn_def.name, f);

                    info!(target: ROOT, fn_name = fn_def.name, file = ?entry.file_name().to_string_lossy(), "register Tera filter");
                });
        }

        Ok(())
    }
}

/// Loco initializer for the Rhai scripting engine with custom setup.
#[derive(Default)]
pub struct ScriptingEngineInitializerWithSetup<F: Fn(&mut Engine) + Send + Sync + 'static> {
    /// Custom setup for the Rhai [`Engine`], if any.
    setup: Option<F>,
}

/// Loco initializer for the Rhai scripting engine.
pub type ScriptingEngineInitializer = ScriptingEngineInitializerWithSetup<fn(&mut Engine)>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptingEngineInitializerConfig {
    /// Directory containing scripts.
    #[serde(default = "ScriptingEngineInitializerConfig::default_scripts_path")]
    pub scripts_path: PathBuf,
    /// Directory containing Tera filters.
    #[serde(default = "ScriptingEngineInitializerConfig::default_filters_path")]
    pub filters_path: PathBuf,
}

impl Default for ScriptingEngineInitializerConfig {
    #[inline(always)]
    fn default() -> Self {
        Self {
            scripts_path: Self::default_scripts_path(),
            filters_path: Self::default_filters_path(),
        }
    }
}

impl ScriptingEngineInitializerConfig {
    /// Default directory containing scripts.
    pub fn default_scripts_path() -> PathBuf {
        SCRIPTS_DIR.into()
    }
    /// Default directory containing Tera filters.
    pub fn default_filters_path() -> PathBuf {
        FILTER_SCRIPTS_DIR.into()
    }
    /// Create a new [`ScriptingEngineInitializerConfig`] instance from the Loco [`AppContext`].
    pub fn from_app_context(ctx: &AppContext) -> Result<Self> {
        let config = ctx
            .config
            .initializers
            .as_ref()
            .and_then(|m| m.get(ScriptingEngineInitializer::NAME))
            .cloned()
            .unwrap_or_default();

        Ok(serde_json::from_value(config)?)
    }
}

impl<F: Fn(&mut Engine) + Send + Sync + 'static> ScriptingEngineInitializerWithSetup<F> {
    /// Initializer name.
    pub const NAME: &'static str = "scripting";

    /// Create a new [`ScriptingEngineInitializerWithSetup`] instance with custom setup for the Rhai [`Engine`].
    #[inline(always)]
    #[must_use]
    pub fn new_with_setup(setup: F) -> Self {
        Self { setup: Some(setup) }
    }
}

#[async_trait]
impl<F: Fn(&mut Engine) + Send + Sync + 'static> Initializer
    for ScriptingEngineInitializerWithSetup<F>
{
    #[inline(always)]
    fn name(&self) -> String {
        Self::NAME.to_string()
    }

    async fn after_routes(&self, router: AxumRouter, ctx: &AppContext) -> Result<AxumRouter> {
        let config = ScriptingEngineInitializerConfig::from_app_context(ctx)?;
        let engine = if let Some(ref setup) = self.setup {
            RhaiScript::new_with_setup(config.scripts_path.clone(), setup)?
        } else {
            RhaiScript::new(config.scripts_path.clone())?
        };

        Ok(router.layer(Extension(ScriptingEngine::from(engine))))
    }
}
