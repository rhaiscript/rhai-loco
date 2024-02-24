Tera filters scripted in Rhai
=============================

This directory contains Tera filters that are scripted in Rhai.

Each Rhai script file (extension `.rhai`) can contain multiple filters.
Sub-directories are ignored.

Each function inside the Rhai script file constitutes one filter, unless marked as `private`.
The name of the function is the name of the filter.


Function Signature
------------------

Each filter function must take exactly _one_ parameter, which is an object-map containing
all the variables in the filter call.

In addition, variables in the filter call can also be accessed as stand-alone variables.

The original data value is mapped to `this`.


Example
-------

For a filter call:

```tera
{{ "hello" | super_duper(a = "world", b = 42, c = true) }}
```

The filter function `super_duper` can be defined as follows in a Rhai script file:

```js
// my_filters.rhai

// This function is ignored
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
