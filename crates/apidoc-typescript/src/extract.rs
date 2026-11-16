//! TypeScript (and JavaScript) source to API items, statically, with the
//! oxc parser. Types are kept as written; nothing is type-checked.

use std::collections::BTreeMap;
use std::path::Path;

use lsdocs_apidoc::{Item, ItemKind, Location, Param, ParamKind, Signature, Visibility};
use lsdocs_plugin_sdk::protocol::{Diagnostic, Severity};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPattern, Class, ClassElement, Declaration, ExportDefaultDeclarationKind, Expression,
    FormalParameters, Function, MethodDefinitionKind, ModuleExportName, PropertyKey, Statement,
    TSAccessibility, TSNamespaceDeclarationBody, TSSignature, TSTypeAnnotation,
    TSTypeParameterDeclaration,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};

/// Values longer than this are cut (`...`).
const MAX_VALUE: usize = 80;

/// One entry of the `paths` option: a file or directory, and optionally
/// the module name it has (an npm package name, say).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
pub(crate) enum Entry {
    /// A path; module names come from it.
    Path(String),
    /// A path and the name of its module (for a file) or the prefix of its
    /// modules' names (for a directory).
    Named {
        /// The path.
        path: String,
        /// The module name.
        module: String,
    },
}

/// What extraction found.
#[derive(Debug, Default)]
pub(crate) struct Found {
    pub items: Vec<Item>,
    pub files: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Extensions of the files read, longest first.
const EXTENSIONS: &[&str] = &[
    ".d.mts", ".d.cts", ".d.ts", ".mts", ".cts", ".tsx", ".ts", ".mjs", ".cjs", ".jsx", ".js",
];

fn stem(name: &str) -> Option<&str> {
    EXTENSIONS.iter().find_map(|ext| name.strip_suffix(ext))
}

/// Extracts the modules at `entries` (relative to `root`): a file is a
/// module (named after the file, or its directory for `index`), a
/// directory every module below it (named by their paths in it).
pub(crate) fn extract(entries: &[Entry], root: &Path) -> Found {
    let mut found = Found::default();
    for entry in entries {
        let (path, name) = match entry {
            Entry::Path(p) => (p.as_str(), None),
            Entry::Named { path, module } => (path.as_str(), Some(module.as_str())),
        };
        let full = root.join(path);
        if full.is_dir() {
            let mut files = Vec::new();
            walk(&full, "", &mut files);
            files.sort();
            for rel in files {
                let Some(module_stem) = stem(&rel) else {
                    continue;
                };
                let module = match (name, module_stem) {
                    (Some(prefix), "index") => prefix.to_owned(),
                    (Some(prefix), s) => format!("{prefix}/{}", s.trim_end_matches("/index")),
                    (None, s) => s.trim_end_matches("/index").to_owned(),
                };
                let shown = format!("{}/{rel}", path.trim_end_matches('/'));
                // A `.d.ts` next to its `.ts` adds nothing.
                if found.items.iter().any(|i| i.path == [module.clone()]) {
                    continue;
                }
                if let Some(item) = module_file(&full.join(&rel), module, &shown, &mut found) {
                    found.items.push(item);
                }
            }
        } else if full.is_file() {
            let file = full
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let module = match name {
                Some(name) => name.to_owned(),
                None => match stem(&file) {
                    Some("index") => full
                        .parent()
                        .and_then(Path::file_name)
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "index".into()),
                    Some(s) => s.to_owned(),
                    None => file.clone(),
                },
            };
            if let Some(item) = module_file(&full, module, path, &mut found) {
                found.items.push(item);
            }
        } else {
            found.diagnostics.push(problem(
                path,
                None,
                format!("no such file or directory: {path}"),
            ));
        }
    }
    found
}

/// Source files under `dir`, relative to it, skipping `node_modules`,
/// hidden entries and tests (`*.test.ts`, `*.spec.ts`).
fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if entry.path().is_dir() {
            walk(&entry.path(), &rel, out);
        } else if stem(&name).is_some_and(|s| !s.ends_with(".test") && !s.ends_with(".spec")) {
            out.push(rel);
        }
    }
}

fn problem(file: &str, line: Option<u32>, message: String) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        message: match line {
            Some(line) => format!("{file}:{line}: {message}"),
            None => message,
        },
        code: Some("apidoc-typescript".into()),
        ..Diagnostic::default()
    }
}

fn line_of(source: &str, at: u32) -> u32 {
    let at = (at as usize).min(source.len());
    let line = source[..at].bytes().filter(|b| *b == b'\n').count() + 1;
    u32::try_from(line).unwrap_or(u32::MAX)
}

fn module_file(file: &Path, module: String, shown: &str, found: &mut Found) -> Option<Item> {
    found.files.push(shown.to_owned());
    let source = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) => {
            found
                .diagnostics
                .push(problem(shown, None, format!("cannot read: {e}")));
            return None;
        }
    };
    let source_type = SourceType::from_path(file).unwrap_or_else(|_| SourceType::ts());
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source, source_type).parse();
    if let Some(error) = parsed.diagnostics.first() {
        let at = error.labels.first().map(|l| l.offset());
        found.diagnostics.push(problem(
            shown,
            at.map(|a| line_of(&source, a)),
            format!("syntax error: {}", error.message),
        ));
        if parsed.fatal_error {
            return None;
        }
    }
    // Doc comments by the position of the token they come before.
    let mut docs = BTreeMap::new();
    for comment in &parsed.program.comments {
        if comment.is_jsdoc() {
            docs.insert(
                comment.attached_to,
                clean(&source[comment.span.start as usize..comment.span.end as usize]),
            );
        }
    }
    let cx = Cx {
        source: &source,
        file: shown,
        docs,
    };
    let mut item = Item::new(ItemKind::Module, vec![module.clone()]);
    item.location = Some(Location {
        file: shown.to_owned(),
        line: None,
    });
    // A file's first doc comment, if a `@module` / `@packageDocumentation`
    // one, documents the module.
    if let Some(first) = parsed.program.comments.iter().find(|c| c.is_jsdoc()) {
        let text = clean(&source[first.span.start as usize..first.span.end as usize]);
        if text.contains("@packageDocumentation")
            || text.contains("@module")
            || text.contains("@file")
            || text.contains("@fileoverview")
        {
            item.docs = Some(strip_module_tags(&text));
        }
    }
    item.members = cx.statements(&parsed.program.body, &[module]);
    Some(item)
}

/// A JSDoc comment's text: delimiters and the leading `*` of each line
/// removed, `{@link}` tags made into roles.
pub(crate) fn clean(comment: &str) -> String {
    let inner = comment.trim_start_matches("/**").trim_end_matches("*/");
    let lines: Vec<String> = inner
        .lines()
        .map(|l| {
            let t = l.trim_start();
            match t.strip_prefix('*') {
                Some(rest) => rest.strip_prefix(' ').unwrap_or(rest).to_owned(),
                None => t.to_owned(),
            }
        })
        .collect();
    links(lines.join("\n").trim())
}

/// `{@link X}`, `{@link X | text}`, `{@link X text}` (and `@linkcode`,
/// `@linkplain`) to `` {js:obj}`text <X>` `` roles, or Markdown links for
/// URLs.
fn links(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{@link") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 6..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let body = after[..end]
            .trim_start_matches("code")
            .trim_start_matches("plain")
            .trim();
        let (target, label) = match body.split_once('|').or_else(|| body.split_once(' ')) {
            Some((t, l)) => (t.trim(), Some(l.trim())),
            None => (body, None),
        };
        if target.contains("://") {
            match label {
                Some(label) => out.push_str(&format!("[{label}]({target})")),
                None => out.push_str(&format!("<{target}>")),
            }
        } else {
            let target = target.replace('#', ".");
            match label {
                Some(label) => out.push_str(&format!("{{js:obj}}`{label} <{target}>`")),
                None => out.push_str(&format!("{{js:obj}}`{target}`")),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn strip_module_tags(text: &str) -> String {
    text.lines()
        .filter(|l| {
            let t = l.trim();
            !(t.starts_with("@packageDocumentation")
                || t.starts_with("@module")
                || t.starts_with("@file"))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

struct Cx<'s> {
    source: &'s str,
    file: &'s str,
    docs: BTreeMap<u32, String>,
}

impl Cx<'_> {
    fn text(&self, span: Span) -> String {
        self.source[span.start as usize..span.end as usize].to_owned()
    }

    /// Text with whitespace runs (newlines too) made single spaces.
    fn flat(&self, span: Span) -> String {
        self.text(span)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn short(&self, span: Span) -> String {
        let flat = self.flat(span);
        if flat.chars().count() > MAX_VALUE {
            format!("{}...", flat.chars().take(MAX_VALUE).collect::<String>())
        } else {
            flat
        }
    }

    fn annotation(
        &self,
        a: &Option<oxc_allocator::Box<'_, TSTypeAnnotation<'_>>>,
    ) -> Option<String> {
        a.as_ref().map(|a| self.flat(a.type_annotation.span()))
    }

    fn generics(
        &self,
        t: &Option<oxc_allocator::Box<'_, TSTypeParameterDeclaration<'_>>>,
    ) -> Vec<String> {
        t.as_ref()
            .map(|t| t.params.iter().map(|p| self.flat(p.span)).collect())
            .unwrap_or_default()
    }

    fn doc(&self, at: u32) -> Option<String> {
        self.docs.get(&at).cloned()
    }

    fn item(&self, kind: ItemKind, scope: &[String], name: &str, span: Span, doc_at: u32) -> Item {
        let mut path = scope.to_vec();
        path.push(name.to_owned());
        let mut item = Item::new(kind, path);
        item.docs = self.doc(doc_at).or_else(|| self.doc(span.start));
        if let Some(docs) = &item.docs
            && ["@internal", "@private", "@hidden", "@ignore"]
                .iter()
                .any(|t| docs.contains(t))
        {
            item.visibility = Visibility::Private;
        }
        item.location = Some(Location {
            file: self.file.to_owned(),
            line: Some(line_of(self.source, span.start)),
        });
        item
    }

    /// The items a module or namespace body declares. Without any export,
    /// everything is public (a script, or a `.d.ts` of globals);
    /// otherwise only what is exported.
    fn statements(&self, body: &[Statement<'_>], scope: &[String]) -> Vec<Item> {
        let has_exports = body.iter().any(|s| {
            matches!(
                s,
                Statement::ExportDeclaration(_)
                    | Statement::ExportNamedDeclaration(_)
                    | Statement::ExportDefaultDeclaration(_)
                    | Statement::ExportAllDeclaration(_)
                    | Statement::ExportFromDeclaration(_)
                    | Statement::TSExportAssignment(_)
            )
        });
        // `export { local as exported }`: local names and what they are
        // exported as.
        let mut exported_names = BTreeMap::new();
        for statement in body {
            if let Statement::ExportNamedDeclaration(e) = statement {
                for spec in &e.specifiers {
                    exported_names.insert(export_name(&spec.local), export_name(&spec.exported));
                }
            }
        }
        let mut items: Vec<Item> = Vec::new();
        for statement in body {
            let (declared, exported) = match statement {
                Statement::ExportDeclaration(e) => {
                    (self.declaration(&e.declaration, scope, e.span.start), true)
                }
                Statement::ExportDefaultDeclaration(e) => {
                    let made = match &e.declaration {
                        ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                            vec![self.function(f, scope, e.span.start, "default")]
                        }
                        ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                            vec![self.class(c, scope, e.span.start, "default")]
                        }
                        ExportDefaultDeclarationKind::TSInterfaceDeclaration(i) => {
                            self.declaration_interface(i, scope, e.span.start)
                        }
                        _ => Vec::new(),
                    };
                    (made, true)
                }
                other => match other.as_declaration() {
                    Some(d) => (self.declaration(d, scope, other.span().start), false),
                    None => continue,
                },
            };
            for mut item in declared {
                if let Some(name) = exported_names.get(item.name()) {
                    // Documented under the name it is exported as.
                    if let Some(last) = item.path.last_mut() {
                        last.clone_from(name);
                    }
                } else if !exported && has_exports {
                    item.visibility = Visibility::Private;
                }
                // Overloads: the first (documented) declaration stays.
                match items
                    .iter_mut()
                    .find(|i| i.path == item.path && i.kind == item.kind)
                {
                    Some(existing) if existing.docs.is_some() || item.docs.is_none() => {}
                    Some(existing) => *existing = item,
                    None => items.push(item),
                }
            }
        }
        items
    }

    fn declaration(&self, d: &Declaration<'_>, scope: &[String], doc_at: u32) -> Vec<Item> {
        match d {
            Declaration::FunctionDeclaration(f) => {
                let name = f.id.as_ref().map_or("default", |i| i.name.as_str());
                vec![self.function(f, scope, doc_at, name)]
            }
            Declaration::ClassDeclaration(c) => {
                let name = c.id.as_ref().map_or("default", |i| i.name.as_str());
                vec![self.class(c, scope, doc_at, name)]
            }
            Declaration::TSInterfaceDeclaration(i) => self.declaration_interface(i, scope, doc_at),
            Declaration::TSTypeAliasDeclaration(t) => {
                let mut item = self.item(
                    ItemKind::TypeAlias,
                    scope,
                    t.id.name.as_str(),
                    t.span,
                    doc_at,
                );
                let generics = self.generics(&t.type_parameters);
                item.ty = Some(self.flat(t.type_annotation.span()));
                // Long types keep their own lines, as written.
                let flat = self.flat(t.type_annotation.span());
                let shown = if flat.len() > 100 {
                    dedent(&self.text(t.type_annotation.span()))
                } else {
                    flat
                };
                item.signature = Some(Signature {
                    text: Some(format!("type {}{} = {shown}", t.id.name, angle(&generics))),
                    generics,
                    ..Signature::default()
                });
                vec![item]
            }
            Declaration::TSEnumDeclaration(e) => {
                let mut item = self.item(ItemKind::Enum, scope, e.id.name.as_str(), e.span, doc_at);
                let prefix = if e.r#const { "const enum" } else { "enum" };
                item.signature = Some(Signature {
                    text: Some(format!("{prefix} {}", e.id.name)),
                    ..Signature::default()
                });
                for member in &e.body.members {
                    let name = self.flat(member.id.span());
                    let name = name.trim_matches(['"', '\'']).to_owned();
                    let mut variant = self.item(
                        ItemKind::Variant,
                        &item.path,
                        &name,
                        member.span,
                        member.span.start,
                    );
                    variant.value = member.initializer.as_ref().map(|i| self.short(i.span()));
                    item.members.push(variant);
                }
                vec![item]
            }
            Declaration::VariableDeclaration(v) => {
                let constant = v.kind.is_const();
                v.declarations
                    .iter()
                    .filter_map(|decl| {
                        let name = binding_name(&decl.id)?;
                        // `const f = (x: T) => ...` documents a function.
                        let function = decl.init.as_ref().and_then(|init| match init {
                            Expression::ArrowFunctionExpression(a) => {
                                Some((&a.type_parameters, &a.params, &a.return_type, a.r#async))
                            }
                            Expression::FunctionExpression(f) => {
                                Some((&f.type_parameters, &f.params, &f.return_type, f.r#async))
                            }
                            _ => None,
                        });
                        let kind = match (&function, constant) {
                            (Some(_), _) => ItemKind::Function,
                            (None, true) => ItemKind::Constant,
                            (None, false) => ItemKind::Variable,
                        };
                        let mut item = self.item(kind, scope, name, decl.span, doc_at);
                        match function {
                            Some((type_parameters, params, returns, is_async)) => {
                                item.signature = Some(self.signature(
                                    name,
                                    if is_async {
                                        "async function"
                                    } else {
                                        "function"
                                    },
                                    type_parameters,
                                    params,
                                    returns,
                                ));
                            }
                            None => {
                                item.ty = self.annotation(&decl.type_annotation);
                                item.value = decl.init.as_ref().map(|i| self.short(i.span()));
                                let keyword = v.kind.as_str();
                                let mut text = format!("{keyword} {name}");
                                if let Some(ty) = &item.ty {
                                    text.push_str(&format!(": {ty}"));
                                }
                                if let Some(value) = &item.value {
                                    text.push_str(&format!(" = {value}"));
                                }
                                item.signature = Some(Signature {
                                    text: Some(text),
                                    ..Signature::default()
                                });
                            }
                        }
                        Some(item)
                    })
                    .collect()
            }
            Declaration::TSNamespaceDeclaration(n) => {
                let mut item = self.item(
                    ItemKind::Namespace,
                    scope,
                    n.id.name.as_str(),
                    n.span,
                    doc_at,
                );
                item.signature = Some(Signature {
                    text: Some(format!("namespace {}", item.qualified("."))),
                    ..Signature::default()
                });
                let mut body = &n.body;
                let mut path = item.path.clone();
                // `namespace a.b { }` nests.
                loop {
                    match body {
                        TSNamespaceDeclarationBody::TSModuleBlock(block) => {
                            item.members = self.statements(&block.body, &path);
                            break;
                        }
                        TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => {
                            path.push(inner.id.name.to_string());
                            body = &inner.body;
                        }
                    }
                }
                vec![item]
            }
            _ => Vec::new(),
        }
    }

    fn declaration_interface(
        &self,
        i: &oxc_ast::ast::TSInterfaceDeclaration<'_>,
        scope: &[String],
        doc_at: u32,
    ) -> Vec<Item> {
        let mut item = self.item(
            ItemKind::Interface,
            scope,
            i.id.name.as_str(),
            i.span,
            doc_at,
        );
        let generics = self.generics(&i.type_parameters);
        item.bases = i.extends.iter().map(|e| self.flat(e.span)).collect();
        let mut text = format!("interface {}{}", i.id.name, angle(&generics));
        if !item.bases.is_empty() {
            text.push_str(&format!(" extends {}", item.bases.join(", ")));
        }
        item.signature = Some(Signature {
            generics,
            text: Some(text),
            ..Signature::default()
        });
        for member in &i.body.body {
            if let Some(m) = self.signature_member(member, &item.path) {
                item.members.push(m);
            }
        }
        vec![item]
    }

    /// A member of an interface (or type literal).
    fn signature_member(&self, member: &TSSignature<'_>, scope: &[String]) -> Option<Item> {
        match member {
            TSSignature::TSPropertySignature(p) => {
                let name = key_name(&p.key, self)?;
                let mut item = self.item(ItemKind::Property, scope, &name, p.span, p.span.start);
                item.ty = self.annotation(&p.type_annotation);
                let mut text = String::new();
                if p.readonly {
                    text.push_str("readonly ");
                }
                text.push_str(&name);
                if p.optional {
                    text.push('?');
                }
                if let Some(ty) = &item.ty {
                    text.push_str(&format!(": {ty}"));
                }
                item.signature = Some(Signature {
                    text: Some(text),
                    ..Signature::default()
                });
                Some(item)
            }
            TSSignature::TSMethodSignature(m) => {
                let name = key_name(&m.key, self)?;
                let mut item = self.item(ItemKind::Method, scope, &name, m.span, m.span.start);
                let shown = if m.optional {
                    format!("{name}?")
                } else {
                    name.clone()
                };
                item.signature =
                    Some(self.signature(&shown, "", &m.type_parameters, &m.params, &m.return_type));
                Some(item)
            }
            TSSignature::TSCallSignatureDeclaration(c) => {
                let mut item = self.item(ItemKind::Method, scope, "(call)", c.span, c.span.start);
                item.signature = Some(Signature {
                    text: Some(self.flat(c.span).trim_end_matches([';', ',']).to_owned()),
                    ..Signature::default()
                });
                Some(item)
            }
            TSSignature::TSConstructSignatureDeclaration(c) => {
                let mut item = self.item(ItemKind::Constructor, scope, "new", c.span, c.span.start);
                item.signature = Some(Signature {
                    text: Some(self.flat(c.span).trim_end_matches([';', ',']).to_owned()),
                    ..Signature::default()
                });
                Some(item)
            }
            TSSignature::TSIndexSignature(s) => {
                let text = self.flat(s.span).trim_end_matches([';', ',']).to_owned();
                let mut item = self.item(ItemKind::Field, scope, "[index]", s.span, s.span.start);
                item.signature = Some(Signature {
                    text: Some(text),
                    ..Signature::default()
                });
                Some(item)
            }
        }
    }

    /// A callable's signature: `prefix name<T>(params): R`.
    fn signature(
        &self,
        name: &str,
        prefix: &str,
        type_parameters: &Option<oxc_allocator::Box<'_, TSTypeParameterDeclaration<'_>>>,
        params: &FormalParameters<'_>,
        returns: &Option<oxc_allocator::Box<'_, TSTypeAnnotation<'_>>>,
    ) -> Signature {
        let generics = self.generics(type_parameters);
        let mut list: Vec<Param> = params
            .items
            .iter()
            .map(|p| {
                let mut name = binding_name(&p.pattern)
                    .map_or_else(|| self.flat(p.pattern.span()), str::to_owned);
                if p.optional {
                    name.push('?');
                }
                Param {
                    name,
                    ty: self.annotation(&p.type_annotation),
                    default: p.initializer.as_ref().map(|i| self.short(i.span())),
                    kind: ParamKind::Normal,
                }
            })
            .collect();
        if let Some(rest) = &params.rest {
            list.push(Param {
                name: binding_name(&rest.rest.argument)
                    .map_or_else(|| self.flat(rest.rest.argument.span()), str::to_owned),
                ty: self.annotation(&rest.type_annotation),
                default: None,
                kind: ParamKind::Variadic,
            });
        }
        let returns = self.annotation(returns);
        let shown: Vec<String> = list
            .iter()
            .map(|p| {
                let mut s = if p.kind == ParamKind::Variadic {
                    format!("...{}", p.name)
                } else {
                    p.name.clone()
                };
                if let Some(ty) = &p.ty {
                    s.push_str(&format!(": {ty}"));
                }
                if let Some(d) = &p.default {
                    s.push_str(&format!(" = {d}"));
                }
                s
            })
            .collect();
        let mut text = String::new();
        if !prefix.is_empty() {
            text.push_str(prefix);
            text.push(' ');
        }
        text.push_str(name);
        text.push_str(&angle(&generics));
        text.push_str(&format!("({})", shown.join(", ")));
        if let Some(r) = &returns {
            text.push_str(&format!(": {r}"));
        }
        Signature {
            generics,
            params: list,
            returns,
            modifiers: Vec::new(),
            text: Some(text),
        }
    }

    fn function(&self, f: &Function<'_>, scope: &[String], doc_at: u32, name: &str) -> Item {
        let mut item = self.item(ItemKind::Function, scope, name, f.span, doc_at);
        let prefix = match (f.r#async, f.generator) {
            (true, true) => "async function*",
            (true, false) => "async function",
            (false, true) => "function*",
            (false, false) => "function",
        };
        item.signature =
            Some(self.signature(name, prefix, &f.type_parameters, &f.params, &f.return_type));
        item
    }

    fn class(&self, c: &Class<'_>, scope: &[String], doc_at: u32, name: &str) -> Item {
        let mut item = self.item(ItemKind::Class, scope, name, c.span, doc_at);
        let generics = self.generics(&c.type_parameters);
        let mut text = String::new();
        if c.r#abstract {
            text.push_str("abstract ");
        }
        text.push_str(&format!("class {name}{}", angle(&generics)));
        if let Some(heritage) = &c.heritage {
            let base = self.flat(heritage.expression.span());
            let base = match &heritage.type_arguments {
                Some(args) => format!("{base}{}", self.flat(args.span)),
                None => base,
            };
            text.push_str(&format!(" extends {base}"));
            item.bases.push(base);
        }
        let implements: Vec<String> = c.implements.iter().map(|i| self.flat(i.span)).collect();
        if !implements.is_empty() {
            text.push_str(&format!(" implements {}", implements.join(", ")));
            item.bases.extend(implements);
        }
        item.signature = Some(Signature {
            generics,
            text: Some(text),
            ..Signature::default()
        });
        for element in &c.body.body {
            match element {
                ClassElement::MethodDefinition(m) => {
                    let Some(member_name) = key_name(&m.key, self) else {
                        continue;
                    };
                    let f = &m.value;
                    let (kind, member_name) = match m.kind {
                        MethodDefinitionKind::Constructor => {
                            (ItemKind::Constructor, "constructor".to_owned())
                        }
                        MethodDefinitionKind::Get | MethodDefinitionKind::Set => {
                            (ItemKind::Property, member_name)
                        }
                        MethodDefinitionKind::Method => (ItemKind::Method, member_name),
                    };
                    let mut member =
                        self.item(kind.clone(), &item.path, &member_name, m.span, m.span.start);
                    member.visibility =
                        visibility(m.accessibility, &m.key).min_with(member.visibility);
                    let mut prefix = Vec::new();
                    if m.r#static {
                        prefix.push("static");
                    }
                    if f.r#async {
                        prefix.push("async");
                    }
                    match m.kind {
                        MethodDefinitionKind::Get => prefix.push("get"),
                        MethodDefinitionKind::Set => prefix.push("set"),
                        _ => {}
                    }
                    let shown = if m.optional {
                        format!("{member_name}?")
                    } else {
                        member_name.clone()
                    };
                    member.signature = Some(self.signature(
                        &shown,
                        &prefix.join(" "),
                        &f.type_parameters,
                        &f.params,
                        &f.return_type,
                    ));
                    if kind == ItemKind::Property {
                        member.ty = self.annotation(&f.return_type);
                        // A getter and its setter are one property.
                        if item.members.iter().any(|x| x.path == member.path) {
                            continue;
                        }
                    }
                    item.members.push(member);
                }
                ClassElement::PropertyDefinition(p) => {
                    let Some(member_name) = key_name(&p.key, self) else {
                        continue;
                    };
                    let mut member = self.item(
                        ItemKind::Field,
                        &item.path,
                        &member_name,
                        p.span,
                        p.span.start,
                    );
                    member.visibility =
                        visibility(p.accessibility, &p.key).min_with(member.visibility);
                    member.ty = self.annotation(&p.type_annotation);
                    member.value = p.value.as_ref().map(|v| self.short(v.span()));
                    let mut text = String::new();
                    if p.r#static {
                        text.push_str("static ");
                    }
                    if p.readonly {
                        text.push_str("readonly ");
                    }
                    text.push_str(&member_name);
                    if p.optional {
                        text.push('?');
                    }
                    if let Some(ty) = &member.ty {
                        text.push_str(&format!(": {ty}"));
                    }
                    if let Some(v) = &member.value {
                        text.push_str(&format!(" = {v}"));
                    }
                    member.signature = Some(Signature {
                        text: Some(text),
                        ..Signature::default()
                    });
                    item.members.push(member);
                }
                _ => {}
            }
        }
        item
    }
}

/// Visibility from TypeScript's accessibility and `#private` names.
fn visibility(accessibility: Option<TSAccessibility>, key: &PropertyKey<'_>) -> Visibility {
    if matches!(key, PropertyKey::PrivateIdentifier(_)) {
        return Visibility::Private;
    }
    match accessibility {
        Some(TSAccessibility::Private) => Visibility::Private,
        Some(TSAccessibility::Protected) => Visibility::Protected,
        _ => Visibility::Public,
    }
}

trait MinWith {
    fn min_with(self, other: Visibility) -> Visibility;
}

impl MinWith for Visibility {
    /// The less visible of two.
    fn min_with(self, other: Visibility) -> Visibility {
        let rank = |v: Visibility| match v {
            Visibility::Public => 0,
            Visibility::Protected => 1,
            Visibility::Internal => 2,
            Visibility::Private => 3,
        };
        if rank(self) >= rank(other) {
            self
        } else {
            other
        }
    }
}

fn key_name(key: &PropertyKey<'_>, cx: &Cx<'_>) -> Option<String> {
    if let PropertyKey::PrivateIdentifier(p) = key {
        return Some(format!("#{}", p.name));
    }
    match key.static_name() {
        Some(name) => name.as_str().map(str::to_owned),
        None => Some(format!("[{}]", cx.flat(key.span()))),
    }
}

fn binding_name<'a>(pattern: &'a BindingPattern<'a>) -> Option<&'a str> {
    pattern.get_identifier_name().map(|n| n.as_str())
}

fn export_name(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(n) => n.name.to_string(),
        ModuleExportName::IdentifierReference(n) => n.name.to_string(),
        ModuleExportName::StringLiteral(s) => s.value.as_str().unwrap_or_default().to_owned(),
    }
}

/// `text` without the indentation its lines after the first share.
fn dedent(text: &str) -> String {
    let indent = text
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out: Vec<&str> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        out.push(if i == 0 {
            line
        } else {
            line.get(indent..).unwrap_or(line.trim_start())
        });
    }
    // Indent continuation lines by two spaces under `type X =`.
    out.iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                (*l).to_owned()
            } else {
                format!("  {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn angle(generics: &[String]) -> String {
    if generics.is_empty() {
        String::new()
    } else {
        format!("<{}>", generics.join(", "))
    }
}
