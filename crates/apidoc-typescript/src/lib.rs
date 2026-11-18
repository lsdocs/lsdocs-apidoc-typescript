//! First-party plugin documenting TypeScript (and JavaScript) APIs from
//! their source code.
//!
//! Reads `.ts`, `.tsx`, `.d.ts` (and `.mts`, `.cts`, `.js`, `.mjs`, ...)
//! files statically, with the oxc parser: types as written, no type
//! checker, nothing run. Documents them with the API framework
//! (`lsdocs-apidoc-plugin`): a page per module under `api/js`, the `js`
//! domain's directives and roles, references between them.
//!
//! ```toml
//! [[plugins]]
//! name = "typescript"
//! builtin = "apidoc-typescript"
//! ```
//!
//! with its settings in `.config/lsdocs/typescript/settings.toml`:
//!
//! ```toml
//! paths = [{ path = "src/index.ts", module = "my-package" }, "src/util"]
//! ```
//!
//! - **Paths** (relative to the site directory): a file is a module, named
//!   after the file (or its directory, for `index`); a directory is every
//!   module below it (named by their paths in it; `node_modules`, hidden
//!   files and `*.test.*` / `*.spec.*` skipped). `{ path, module }` names
//!   a file's module (or prefixes a directory's), e.g. after its npm
//!   package.
//! - **What is documented:** what a module exports (`export` declarations,
//!   `export { a, b }`, `export default`); in a file without exports
//!   (a script, or a `.d.ts` of globals), everything. Functions (and
//!   `const f = (...) => ...`), classes with their constructors, methods,
//!   accessors and fields, interfaces with their members, type aliases,
//!   enums, constants and variables, namespaces. `private` and
//!   `#private` members are private, `protected` ones protected; doc
//!   comments tagged `@internal`, `@private`, `@hidden` or `@ignore` too.
//! - **Doc comments** (`/** ... */`) are TSDoc / JSDoc: Markdown, with
//!   `@param`, `@returns`, `@throws`, `@example`, `@deprecated` and the
//!   like; `{@link Name}` becomes a reference. A file's first comment
//!   documents the module if it has `@packageDocumentation` or `@module`.
//! - **The `js` domain:** `js:function`, `js:method`, `js:class`,
//!   `js:interface`, `js:type`, `js:enum`, `js:data`, `js:attribute`,
//!   `js:namespace`, `js:module`, `js:currentmodule`, `js:auto`; roles
//!   `js:func`, `js:meth`, `js:class`, `js:interface`, `js:type`,
//!   `js:enum`, `js:attr`, `js:data`, `js:mod`, `js:obj`.
//!
//! Settings: `paths` (required for generated pages) and those of the
//! framework (`output`, `private`, `title`, `doc_format`, `doc_style`,
//! `generate`, `primary`).

mod extract;
#[cfg(test)]
mod tests;

use std::path::Path;

use lsdocs_apidoc::{DocStyle, ItemKind};
use lsdocs_apidoc_plugin::{Extracted, Language};
use lsdocs_plugin_sdk::Plugin;
use serde_json::Value;

/// The plugin, as registered with the host.
pub fn plugin() -> Plugin {
    lsdocs_apidoc_plugin::plugin(TypeScript, env!("CARGO_PKG_VERSION"))
}

/// TypeScript and JavaScript.
#[derive(Debug, Clone, Copy, Default)]
pub struct TypeScript;

impl Language for TypeScript {
    fn domain(&self) -> &str {
        "js"
    }

    fn name(&self) -> &str {
        "TypeScript"
    }

    fn code_language(&self) -> &str {
        "typescript"
    }

    fn settings(&self) -> &[&str] {
        &["paths"]
    }

    fn doc_format(&self) -> &str {
        "md"
    }

    fn doc_style(&self) -> DocStyle {
        DocStyle::Tags
    }

    fn extract(&self, options: &Value, root: &Path) -> Result<Extracted, String> {
        let entries: Vec<extract::Entry> = match options.get("paths") {
            Some(paths) => serde_json::from_value(paths.clone()).map_err(|e| {
                format!("`paths` must be a list of paths or {{ path, module }} tables: {e}")
            })?,
            None => return Err("the `paths` option is missing".into()),
        };
        let found = extract::extract(&entries, root);
        Ok(Extracted {
            items: found.items,
            files: found.files,
            diagnostics: found.diagnostics,
        })
    }

    fn objects(&self) -> Vec<(String, ItemKind)> {
        [
            ("function", ItemKind::Function),
            ("method", ItemKind::Method),
            ("class", ItemKind::Class),
            ("interface", ItemKind::Interface),
            ("type", ItemKind::TypeAlias),
            ("enum", ItemKind::Enum),
            ("data", ItemKind::Constant),
            ("attribute", ItemKind::Property),
            ("namespace", ItemKind::Namespace),
        ]
        .into_iter()
        .map(|(n, k)| (n.to_owned(), k))
        .collect()
    }

    fn roles(&self) -> Vec<(String, Vec<ItemKind>)> {
        use ItemKind::*;
        [
            ("func", vec![Function, Method, Constructor]),
            ("meth", vec![Method, Constructor, Function]),
            ("class", vec![Class, Interface]),
            ("interface", vec![Interface]),
            ("type", vec![TypeAlias, Interface, Class, Enum]),
            ("enum", vec![Enum]),
            ("attr", vec![Property, Field, Variant]),
            ("data", vec![Constant, Variable]),
            ("mod", vec![Module, Namespace]),
            ("obj", vec![]),
        ]
        .into_iter()
        .map(|(n, k)| (n.to_owned(), k))
        .collect()
    }
}
