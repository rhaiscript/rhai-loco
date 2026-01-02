Rhai Engine Integration for Loco
================================

![GitHub last commit](https://img.shields.io/github/last-commit/rhaiscript/rhai-loco?logo=github)
[![Stars](https://img.shields.io/github/stars/rhaiscript/rhai-loco?style=flat&logo=github)](https://github.com/rhaiscript/rhai-loco)
[![License](https://img.shields.io/crates/l/rhai-loco)](https://github.com/license/rhaiscript/rhai-loco)
[![crates.io](https://img.shields.io/crates/v/rhai-loco?logo=rust)](https://crates.io/crates/rhai-loco/)
[![crates.io](https://img.shields.io/crates/d/rhai-loco?logo=rust)](https://crates.io/crates/rhai-loco/)
[![API Docs](https://docs.rs/rhai-loco/badge.svg?logo=docs-rs)](https://docs.rs/rhai-loco/)

This crate adds [Rhai](https://rhai.rs) script support to [Loco](https://loco.rs).


Why Include a Scripting Engine
------------------------------

Although a system based upon [Loco](https://loco.rs) is usually compiled for maximum performance, there are times where user requirements are dynamic and need to be adapted to, preferably without recompilation.

Scripts are tremendously useful in the following cases:

* Complex custom configuration or custom business logic per installation at different sites without recompilation. In a different programming language, DLL's or dynamically-linked libraries may be used.

* Rapidly adapt to changing environments (e.g. handle new data formats, input changes, or novel user errors etc.) without hard-coding the rules (which may soon change again).

* Trial testing new features or business logic with fast iteration (without recompilation). The final version, once stable, can be converted into native Rust code for performance.

* Develop [Tera](https://crates.io/crates/tera) filters in script so they can be iterated quickly. Useful ones can then be converted into Rust native filters. This can normally be achieved via [Tera](https://crates.io/crates/tera) macros, but the Rhai scripting language is more powerful and expressive than [Tera](https://crates.io/crates/tera) expressions, allowing more complex logic to be implemented.


Usage
-----

Import `rhai-loco` inside `Cargo.toml`:

```toml
[dependencies]
rhai-loco = "0.15.0"
```


Configuration
-------------

The Loco `config` section of `initializers` can be used to set options for the Rhai engine.

```yaml
# Initializers configuration
initializers:
  # Scripting engine configuration
  scripting:
    # Directory holding scripts
    scripts_path: assets/scripts
    # Directory holding Tera filter scripts
    filters_path: assets/scripts/tera/filters
```


Enable Scripted Tera Filters
----------------------------

Modify the `ViewEngineInitializer` under `src/initializers/view_engine.rs`:

```rust
┌─────────────────────────────────┐
│ src/initializers/view_engine.rs │
└─────────────────────────────────┘

///////////////////////////////////////////////////////////////////////////////////
// Within this method...
// Modify as follows to enable scripted Tera filters.
async fn after_routes(&self, router: AxumRouter, _ctx: &AppContext) -> Result<AxumRouter> {
    /////////////////////////////////////////////////////////////////
    // Add code to get scripting engine configuration
    let config = ScriptingEngineInitializerConfig::from_app_context(_ctx)?;
    let filters_path = config.filters_path.is_dir().then_some(config.filters_path);
    // End modification
    /////////////////////////////////////////////////////////////////

    let tera_engine = if std::path::Path::new(I18N_DIR).exists() {
        let arc = std::sync::Arc::new(
            ArcLoader::builder(&I18N_DIR, unic_langid::langid!("en-US"))
                .shared_resources(Some(&[I18N_SHARED.into()]))
                .customize(|bundle| bundle.set_use_isolating(false))
                .build()
                .map_err(|e| Error::string(&e.to_string()))?,
        );
        info!("locales loaded");

        engines::TeraView::build()?.post_process(move |tera| {
            ///////////////////////////////////////////////////////////////
            // Add Rhai scripted filters registration (with i18n)
            if let Some(ref path) = filters_path {
                rhai_loco::RhaiScript::register_tera_filters(tera, &path,
                    |_engine| {},   // custom configuration of the Rhai Engine, if any
                    FluentLoader::new(arc.clone()),
                )?;
                info!("Filter scripts loaded");
            }
            // End modification
            ///////////////////////////////////////////////////////////////
            tera.register_function("t", FluentLoader::new(arc.clone()));
            Ok(())
        })?
    /////////////////////////////////////////////////
    // Add Rhai scripted filters registration (no i18n)
    } else if let Some(path) = filters_path {
        engines::TeraView::build()?.post_process(move |tera| {
            rhai_loco::RhaiScript::register_tera_filters(tera, &path,
                |_engine| {},   // custom configuration of the Rhai Engine, if any
                FluentLoader::new(arc.clone()),
            )?;
            info!("Filter scripts loaded");
            Ok(())
        })?
    // End modification
    /////////////////////////////////////////////////
    } else {
        engines::TeraView::build()?
    };

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

### Scripted filters as conversion/formatting tool

Scripted filters can be very flexible for ad-hoc conversion/formatting purposes because they enable
rapid iterations and changes without recompiling.

```rust
┌────────────────────┐
│ Rhai filter script │
└────────────────────┘

/// Say we have in-house status codes that we need to convert into text
/// for display with i18n support...
fn status(vars) {
    switch this {
        case "P" => t("Pending", lang),
        case "A" => t("Active", lang),
        case "C" => t("Cancelled", lang),
        case "X" => t("Deleted", lang),
    }
}

/// Use script to inject HTML also!
/// The input value is used to select from the list of options
fn all_status(vars) {`
    <option value="P" ${if this == "P" { "selected" }}>t("Pending", lang)</option>
    <option value="A" ${if this == "A" { "selected" }}>t("Active", lang)</option>
    <option value="C" ${if this == "C" { "selected" }}>t("Cancelled", lang)</option>
    <option value="X" ${if this == "X" { "selected" }}>t("Deleted", lang)</option>
`}

/// Say we have CSS classes that we need to add based on certain data values
fn count_css(vars) {
    if this.count > 1 {
        "error more-than-one"
    } else if this.count == 0 {
        "error missing-value"
    } else {
        "success"
    }
}
```

```html
┌───────────────┐
│ Tera template │
└───────────────┘

<!-- use script to determine the CSS class -->
<div id="record" class="{{ value | count_css }}">
    <!-- use script to map the status display -->
    <span>{{ value.status | status(lang="de-DE") }} : {{ value.count }}</span>
</div>

<!-- use script to inject HTML directly -->
<select>
    <option value="">t("All", "de-DE")</option>
    <!-- avoid escaping as text via the `safe` filter -->
    {{ "A" | all_status(lang="de-DE") | safe }}
</select>
```

The above is equivalent to the following Tera template.

Technically speaking, you either maintain such ad-hoc behavior in script or inside the Tera template
itself, but doing so in script allows for reuse and a cleaner template.

```html
┌───────────────┐
│ Tera template │
└───────────────┘

<div id="record" class="{% if value.count > 1 %}
                            error more-than-one
                        {% elif value.count == 0 %}
                            error missing-value
                        {% else %}
                            success
                        {% endif %}">

    <span>
        {% if value.status == "P" %}
            t(key = "Pending", lang = "de-DE")
        {% elif value.status == "A" %}
            t(key = "Active", lang = "de-DE")
        {% elif value.status == "C" %}
            t(key = "Cancelled", lang = "de-DE")
        {% elif value.status == "D" %}
            t(key = "Deleted", lang = "de-DE")
        {% endif %}
        : {{ value.count }}
    </span>
</div>
```


Run a Rhai script in Loco Request
---------------------------------

The scripting engine is first injected into Loco via the `ScriptingEngineInitializer`:

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
support, it is easy to perform custom setup on the Rhai engine via `ScriptingEngineInitializerWithSetup`:

```rust
┌────────────┐
│ src/app.rs │
└────────────┘

async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
    Ok(vec![
        // Add the scripting engine initializer
        Box::new(rhai_loco::ScriptingEngineInitializerWithSetup::new_with_setup(|engine| {
                        :
            // ... do custom setup of Rhai engine here ...
                        :
        })),
        Box::new(initializers::view_engine::ViewEngineInitializer),
    ])
}
```
