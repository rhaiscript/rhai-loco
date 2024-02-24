Rhai Engine Integration for Loco
================================

![GitHub last commit](https://img.shields.io/github/last-commit/rhaiscript/rhai-loco?logo=github)
[![Stars](https://img.shields.io/github/stars/rhaiscript/rhai-loco?style=flat&logo=github)](https://github.com/rhaiscript/rhai-loco)
[![License](https://img.shields.io/crates/l/rhai-loco)](https://github.com/license/rhaiscript/rhai-loco)
[![crates.io](https://img.shields.io/crates/v/rhai-loco?logo=rust)](https://crates.io/crates/rhai-loco/)
[![crates.io](https://img.shields.io/crates/d/rhai-loco?logo=rust)](https://crates.io/crates/rhai-loco/)
[![API Docs](https://docs.rs/rhai-loco/badge.svg?logo=docs-rs)](https://docs.rs/rhai-loco/)

This crate adds [Rhai](https://rhai.rs) script support to [Loco](https://loco.rs).


Usage
-----

Import `rhai-loco` inside `Cargo.toml`:

```toml
[dependencies]
rhai-loco = "0.1.0"
```


Enable Scripted Tera Filters
----------------------------

Modify the `ViewEngineInitializer` under `src/initializers/view_engine.rs`:

```rust
┌─────────────────────────────────┐
│ src/initializers/view_engine.rs │
└─────────────────────────────────┘

// Within this method...
async fn after_routes(&self, router: AxumRouter, _ctx: &AppContext) -> Result<AxumRouter> {
    let mut tera_engine = engines::TeraView::build()?;

    if Path::new(I18N_DIR).exists() {
        debug!("locales dir = {I18N_DIR}");
        let arc = ArcLoader::builder(I18N_DIR, unic_langid::langid!("en-US"))
            .shared_resources(Some(&[I18N_SHARED.into()]))
            .customize(|bundle| bundle.set_use_isolating(false))
            .build()
            .map_err(|e| Error::string(&e.to_string()))?;
        tera_engine
            .tera
            .register_function("t", FluentLoader::new(arc));
        info!("locales loaded");
    }

    // Add the following to enable scripted Tera filters >>>
    let path = Path::new("assets/scripts/tera/filters");
    if path.exists() {
        rhai_loco::RhaiScript::register_tera_filters(&mut tera_engine, path)?;
    }
    // <<< End addition

    Ok(router.layer(Extension(ViewEngine::from(tera_engine))))
}
```

Each Rhai script file (extension `.rhai`) can contain multiple filters. Sub-directories are ignored.

Each function inside the Rhai script file constitutes one filter, unless marked as `private`.
The name of the function is the name of the filter.


### Function Signature

Each filter function must take exactly _one_ parameter, which is an object-map containing all the
variables in the filter call.

In addition, variables in the filter call can also be accessed as stand-alone variables.

The original data value is mapped to `this`.


### Example

For a filter call:

```tera
┌───────────────┐
│ Tera template │
└───────────────┘

{{ "hello" | super_duper(a = "world", b = 42, c = true) }}
```

The filter function `super_duper` can be defined as follows in a Rhai script file:

```js
┌─────────────┐
│ Rhai script │
└─────────────┘

// This private function is ignored
private fn do_something(x) {
    ...
}

// This function has the wrong number of parameters and is ignored
fn do_other_things(x, y, z) {
    ...
}

// Filter 'super_duper'
fn super_duper(vars) {
    // 'this' maps to "hello"
    // 'vars' contains 'a', 'b' and 'c'
    // The stand-alone variables 'a', 'b' and 'c' can also be accessed

    let name = if vars.b > 0 {  // access 'b' under 'vars'
        ...
    } else if c {               // access 'c'
        ...
    } else !a.is_empty() {      // access 'a'
        ...
    } else {
        ...
    }

    // 'this' can be modified
    this[0].to_upper();

    // Return new value
    `${this}, ${name}!`
}
```


Run a Rhai script in Loco Request
---------------------------------

The scripting engine is first injected into Loco via an initializer:

```rust
┌────────────┐
│ src/app.rs │
└────────────┘

async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
    Ok(vec![
        // Add the scripting engine initializer
        Box::new(rhai_loco::ScriptingEngineInitializer),
        Box::new(initializers::view_engine::ViewEngineInitializer),
    ])
}
```

The scripting engine can then be extracted in requests using `ScriptingEngine`.

For example, the following adds custom scripting support to the login authentication process:

```rust
┌─────────────────────────┐
│ src/controllers/auth.rs │
└─────────────────────────┘

// Import the scripting engine types
use rhai_loco::{RhaiScript, ScriptingEngine};

pub async fn login(
    State(ctx): State<AppContext>,
    // Extract the scripting engine
    ScriptingEngine(script): ScriptingEngine<RhaiScript>,
    Json(mut params): Json<LoginParams>,
) -> Result<Json<LoginResponse>> {
    // Use `run_script_if_exists` to run a function `login` from a script
    // `on_login.rhai` if it exists under `assets/scripts/`.
    //
    // Use `run_script` if the script is required to exist or an error is returned.
    let result = script
        .run_script_if_exists("on_login", &mut params, "login", ())
        //                    ^ script file            ^ function name
        //                                ^ data mapped to `this` in script
        //                                                      ^^ function arguments
        .or_else(|err| script.convert_runtime_error(err, |msg| unauthorized(&msg)))?;
        //                                               ^^^^^^^^^^^^^^^^^^^^^^^^
        //                      turn any runtime error into an unauthorized response

                :
                :
}
```

This calls a function named `login` within the script file `on_login.rhai` if it exists:

```rust
┌──────────────────────────────┐
│ assets/scripts/on_login.rhai │
└──────────────────────────────┘

// Function for custom login logic
fn login() {
    // Can import other Rhai modules!
    import "super/secure/vault" as vault;

    debug(`Trying to login with user = ${this.user} and password = ${this.password}`);

    let security_context = vault.extensive_checking(this.user, this.password);

    if security_context.passed {
        // Data values can be changed!
        this.user = security_context.masked_user;
        this.password = security_context.masked_password;
        return security_context.id;
    } else {
        vault::black_list(this.user);
        throw `The user ${this.user} has been black-listed!`;
    }
}
```


Custom Engine Setup
-------------------

In order to customize the Rhai scripting engine, for example to add custom functions or custom types
support, it is easy define a custom initializer based on the existing template:

```rust
use axum::{Router as AxumRouter, Extension};
use loco_rs::app::AppContext;
use loco_rs::prelude::*;
use rhai_loco::RhaiScript;

pub struct CustomScriptingEngineInitializer;

pub const SCRIPTS_DIR: &'static str = "assets/scripts/";

#[async_trait]
impl Initializer for CustomScriptingEngineInitializer {
    fn name(&self) -> String {
        "scripting-engine".to_string()
    }

    async fn after_routes(&self, router: AxumRouter, _ctx: &AppContext) -> Result<AxumRouter> {
        // Use `RhaiScript::new_with_setup` to customize the Rhai engine
        let engine = RhaiScript::new_with_setup(SCRIPTS_DIR, |engine| {
                        :
            // ... do custom setup of Rhai engine here ...
                        :
        })?;

        Ok(router.layer(Extension(ScriptingEngine::from(engine))))
    }
}
```
