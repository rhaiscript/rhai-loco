use axum::{
    async_trait, extract::FromRequestParts, http::request::Parts, Extension, Router as AxumRouter,
};
use loco_rs::app::{AppContext, Initializer};
use loco_rs::prelude::*;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fmt::Debug,
    fs::read_dir,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
};
use tracing::{debug, info, trace_span};

// Re-export useful Rhai types and functions.
pub use rhai::{
    eval, eval_file, format_map_as_json, run, run_file,
    serde::{from_dynamic, to_dynamic},
    Array, Blob, CallFnOptions, Dynamic, Engine, EvalAltResult, EvalContext, FnAccess, FnNamespace,
    FnPtr, FuncArgs, FuncRegistration, ImmutableString, Instant, LexError, Map, Module,
    NativeCallContext, OptimizationLevel, ParseError, ParseErrorType, Position, Scope,
    ScriptFnMetadata, VarDefInfo, AST,
};
pub type RhaiResult<T> = std::result::Result<T, Box<EvalAltResult>>;
use rhai::module_resolvers::FileModuleResolver;

/// Global Rhai [`Engine`] instance.
static ENGINE: OnceLock<Engine> = OnceLock::new();

/// Get a 'static reference to the Rhai [`Engine`].
pub fn get_engine() -> &'static Engine {
    ENGINE.get_or_init(|| Engine::new())
}
/// Set up the Rhai [`Engine`] with extra setup.
fn setup_engine(setup: impl FnOnce(&mut Engine)) -> &'static Engine {
    ENGINE.get_or_init(|| {
        let mut engine = Engine::new();
        setup(&mut engine);
        engine
    })
}

static FILTERS_ENGINE: OnceLock<Engine> = OnceLock::new();

const ERR_MSG_SCRIPT_FILE_NOT_FOUND: &str = "script file not found";

/// Type that wraps a scripting engine.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ScriptingEngine<E>(pub E);

impl<E> ScriptingEngine<E> {
    /// Creates a new [`ScriptingEngine`] that wraps the given scripting engine
    pub fn new(engine: E) -> Self {
        Self(engine)
    }
}

impl<E> From<E> for ScriptingEngine<E> {
    fn from(inner: E) -> Self {
        Self::new(inner)
    }
}

/// A scripting engine based on [`Rhai`](https://rhai.rs).
#[derive(Debug, Clone)]
pub struct RhaiScript {
    /// Path to the directory containing Rhai scripts.
    scripts_path: PathBuf,
    /// Cache of compiled Rhai scripts in [`AST`] form.
    cache: Arc<RwLock<HashMap<PathBuf, Arc<AST>>>>,
}

impl RhaiScript {
    /// File extension for Rhai scripts.
    pub const SCRIPTS_EXT: &'static str = "rhai";

    /// Create a new [`RhaiScript`] instance.
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
    /// # Errors
    ///
    /// Error if the scripts directory does not exist.
    pub fn new_with_setup(
        scripts_path: impl Into<PathBuf>,
        setup: impl FnOnce(&mut Engine),
    ) -> Result<Self> {
        assert!(
            ENGINE.get().is_none(),
            "`RhaiEngine::new_with_setup` can only be called once."
        );

        let scripts_path = scripts_path.into();

        if !scripts_path.exists() {
            return Err(Error::string(&format!(
                "missing scripts directory: `{}`",
                scripts_path.to_string_lossy()
            )));
        }

        let _ = setup_engine(|engine| {
            engine
                .set_module_resolver(FileModuleResolver::new_with_path_and_extension(
                    scripts_path.clone(),
                    Self::SCRIPTS_EXT,
                ))
                .on_print(|message| info!(message))
                .on_debug(|message, source, pos| debug!(?message, source, position = ?pos));

            setup(engine);
        });

        Ok(Self {
            scripts_path,
            cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Get a 'static reference to the Rhai [`Engine`].
    #[inline(always)]
    #[must_use]
    pub fn engine(&self) -> &'static Engine {
        get_engine()
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
                    if s == ERR_MSG_SCRIPT_FILE_NOT_FOUND && e.to_string() == script_file =>
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
        let mut path = self.scripts_path.join(script_file);

        if path.extension().is_none() {
            path.set_extension(Self::SCRIPTS_EXT);
        }

        let span = trace_span!("run_script");
        let _ = span.enter();

        if !path.exists() {
            debug!(file = script_file, ERR_MSG_SCRIPT_FILE_NOT_FOUND);
            return Err(EvalAltResult::ErrorSystem(
                ERR_MSG_SCRIPT_FILE_NOT_FOUND.to_string(),
                script_file.into(),
            )
            .into());
        }

        let mut cache = self.cache.write().unwrap();

        let ast = if let Some(ast) = cache.get(&path) {
            ast
        } else {
            let mut ast = self.engine().compile_file(path.clone())?;
            ast.set_source(path.to_string_lossy().as_ref());
            cache.entry(path).or_insert_with(|| Arc::new(ast.clone()))
        };

        let source = ast.source();
        debug!(fn_name, ?data, source, "Rhai: call function");

        let mut obj = to_dynamic(&*data).unwrap();
        let options = CallFnOptions::new().bind_this_ptr(&mut obj);

        let result = self
            .engine()
            .call_fn_with_options(options, &mut Scope::new(), ast, fn_name, args)
            .map_err(|err| match *err {
                EvalAltResult::ErrorInFunctionCall(f, _, e, Position::NONE) if f == fn_name => e,
                _ => err,
            });

        *data = from_dynamic(&obj).unwrap();

        debug!(?result, ?data, fn_name, source, "Rhai: function returns");

        result
    }

    /// Register Tera filters from Rhai scripts.
    ///
    /// # Errors
    ///
    /// * Error if the filter scripts directory does not exist.
    /// * Error if there is a syntax error in any script during compilation.
    pub fn register_tera_filters(
        tera: &mut TeraView,
        scripts_path: impl AsRef<Path>,
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
            engine
                .on_print(|message| info!(message))
                .on_debug(|message, source, pos| debug!(?message, source, position = ?pos));
            engine
        });

        for entry in read_dir(path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                debug!(dir = ?entry.file_name().to_string_lossy(), "skip dir");
                continue;
            } else if path
                .extension()
                .map_or(true, |ext| ext.to_string_lossy() != Self::SCRIPTS_EXT)
            {
                debug!(file = ?entry.file_name().to_string_lossy(), "skip non-script file");
                continue;
            }

            let mut ast = engine.compile_file(path.clone()).map_err(Error::msg)?;
            ast.set_source(path.to_string_lossy().as_ref());
            let ast = Arc::new(ast);
            debug!(file = ?entry.file_name().to_string_lossy(), "compile script");

            ast.iter_functions()
                .filter(|fn_def| fn_def.access != FnAccess::Private && fn_def.params.len() == 1)
                .for_each(|fn_def| {
                    let fn_name = fn_def.name.to_string();
                    let ast = ast.clone();

                    let f = move |value: &Value,
                                  variables: &HashMap<String, Value>|
                          -> tera::Result<Value> {
                        debug!(fn_name, ?value, ?variables, "Rhai: call Tera filter");

                        let mut obj = to_dynamic(value).unwrap();
                        let dict = to_dynamic(variables).unwrap().cast::<Map>();

                        let scope = &mut Scope::new();
                        dict.iter().for_each(|(k, v)| {
                            scope.push_dynamic(k.clone(), v.clone());
                        });

                        let options = CallFnOptions::new().bind_this_ptr(&mut obj);
                        let value = engine
                            .call_fn_with_options::<Dynamic>(
                                options,
                                scope,
                                &ast,
                                &fn_name,
                                (dict,),
                            )
                            .map_err(tera::Error::msg)?;

                        let value = from_dynamic(&value).unwrap();
                        debug!(
                            ?value,
                            fn_name,
                            ?variables,
                            "Rhai: return value from Tera filter"
                        );

                        Ok(value)
                    };

                    tera.tera.register_filter(fn_def.name, f);
                    info!(fn_name = fn_def.name, "register Tera filter");
                });
        }

        Ok(())
    }
}

#[async_trait]
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

pub struct ScriptingEngineInitializer;

pub const SCRIPTS_DIR: &'static str = "assets/scripts/";

#[async_trait]
impl Initializer for ScriptingEngineInitializer {
    fn name(&self) -> String {
        "scripting-engine".to_string()
    }

    async fn after_routes(&self, router: AxumRouter, _ctx: &AppContext) -> Result<AxumRouter> {
        let engine = RhaiScript::new(SCRIPTS_DIR)?;
        Ok(router.layer(Extension(ScriptingEngine::from(engine))))
    }
}