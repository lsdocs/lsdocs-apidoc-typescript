#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use lsdocs_apidoc::{Item, ItemKind, Visibility};
use lsdocs_apidoc_plugin::{Language, default_signature};
use lsdocs_plugin_sdk::doctree::{Node, plain_text};
use lsdocs_plugin_sdk::protocol::{InitializeParams, SiteInfo};
use lsdocs_plugin_sdk::testing::{context_with_parsers, run_transform_in};
use serde_json::json;

use super::extract::{Entry, clean, extract};
use super::{TypeScript, plugin};

fn site(label: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lsdocs-ts-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

const INDEX: &str = r#"/**
 * The *shapes* package.
 * @packageDocumentation
 */

/** A point. */
export interface Point {
  /** Across. */
  readonly x: number;
  y?: number;
  /** Moves it. */
  move(dx: number, dy?: number): Point;
  [key: string]: unknown;
}

/** Kinds of shapes. */
export enum Kind {
  /** Round. */
  Circle = "circle",
  Square = "square",
}

/** Anything with an area, see {@link area}. */
export type Shape<T = number> = { area(): T };

/**
 * A circle.
 * @deprecated Use {@link Disk | disks}.
 */
export class Circle<T> extends Base<T> implements Shape {
  /** How many were made. */
  static count = 0;
  #secret = 1;
  private hidden: string;
  protected shared = true;

  /**
   * Makes one.
   * @param radius - How big.
   */
  constructor(public radius: number) {
    super();
  }

  /** Twice the radius. */
  get diameter(): number {
    return 2 * this.radius;
  }
  set diameter(d: number) {}

  /**
   * Grows it.
   * @param by - How much.
   * @returns The new radius.
   * @throws {RangeError} If negative.
   */
  async grow(by: number, ...more: number[]): Promise<number> {
    return this.radius;
  }
}

/**
 * The area of a shape.
 * @param shape - The shape.
 * @example
 * ```ts
 * area(new Circle(1))
 * ```
 */
export function area(shape: Shape): number;
export function area(shape: unknown): number {
  return 0;
}

/** Doubles. */
export const double = (x: number): number => 2 * x;

/** The origin. */
export const ORIGIN: Point = { x: 0, y: 0 };

/** @internal */
export function internal(): void {}

function helper(): void {}

export { helper as default2 };

/** Geometry helpers. */
export namespace geo {
  /** Distance. */
  export function dist(a: Point, b: Point): number {
    return 0;
  }
  function hidden(): void {}
}
"#;

fn find<'a>(items: &'a [Item], path: &str) -> &'a Item {
    items
        .iter()
        .flat_map(Item::walk)
        .find(|i| i.qualified(".") == path)
        .unwrap_or_else(|| panic!("no {path}"))
}

fn sig(item: &Item) -> String {
    default_signature(item, ".")
}

#[test]
fn extracts_exports() {
    let dir = site(
        "x",
        &[
            ("src/index.ts", INDEX),
            (
                "src/util/strings.ts",
                "/** Upper. */\nexport function up(s: string): string { return s; }\n",
            ),
            ("src/util/strings.test.ts", "export function test() {}\n"),
            (
                "types/globals.d.ts",
                "/** A global. */\ndeclare function glob(x: number): void;\n",
            ),
            ("bad.ts", "export function (\n"),
        ],
    );
    let found = extract(
        &[
            Entry::Named {
                path: "src".into(),
                module: "shapes".into(),
            },
            Entry::Path("types/globals.d.ts".into()),
            Entry::Path("bad.ts".into()),
        ],
        &dir,
    );
    let _ = std::fs::remove_dir_all(&dir);
    let modules: Vec<String> = found.items.iter().map(|i| i.qualified(".")).collect();
    assert_eq!(modules, ["shapes", "shapes/util/strings", "globals"]);
    assert_eq!(found.diagnostics.len(), 1, "{:?}", found.diagnostics);
    assert!(
        found.diagnostics[0]
            .message
            .starts_with("bad.ts:1: syntax error"),
        "{:?}",
        found.diagnostics
    );
    let items = &found.items;
    assert_eq!(items[0].docs.as_deref(), Some("The *shapes* package."));
    let point = find(items, "shapes.Point");
    assert_eq!(sig(point), "interface Point");
    let members: Vec<String> = point.members.iter().map(sig).collect();
    assert_eq!(
        members,
        [
            "readonly x: number",
            "y?: number",
            "move(dx: number, dy?: number): Point",
            "[key: string]: unknown"
        ]
    );
    assert_eq!(
        find(items, "shapes.Point.x").docs.as_deref(),
        Some("Across.")
    );
    assert_eq!(sig(find(items, "shapes.Kind")), "enum Kind");
    assert_eq!(
        find(items, "shapes.Kind.Circle").value.as_deref(),
        Some("\"circle\"")
    );
    assert_eq!(
        find(items, "shapes.Kind.Circle").docs.as_deref(),
        Some("Round.")
    );
    let shape = find(items, "shapes.Shape");
    assert_eq!(sig(shape), "type Shape<T = number> = { area(): T }");
    assert_eq!(
        shape.docs.as_deref(),
        Some("Anything with an area, see {js:obj}`area`.")
    );
    let circle = find(items, "shapes.Circle");
    assert_eq!(
        sig(circle),
        "class Circle<T> extends Base<T> implements Shape"
    );
    assert!(
        circle
            .docs
            .as_deref()
            .unwrap()
            .contains("@deprecated Use {js:obj}`disks <Disk>`.")
    );
    let members: Vec<(String, Visibility)> = circle
        .members
        .iter()
        .map(|m| (sig(m), m.visibility))
        .collect();
    assert_eq!(
        members,
        [
            ("static count = 0".to_owned(), Visibility::Public),
            ("#secret = 1".to_owned(), Visibility::Private),
            ("hidden: string".to_owned(), Visibility::Private),
            ("shared = true".to_owned(), Visibility::Protected),
            ("constructor(radius: number)".to_owned(), Visibility::Public),
            ("get diameter(): number".to_owned(), Visibility::Public),
            (
                "async grow(by: number, ...more: number[]): Promise<number>".to_owned(),
                Visibility::Public
            ),
        ]
    );
    assert_eq!(
        find(items, "shapes.Circle.constructor").kind,
        ItemKind::Constructor
    );
    // Overloads: the documented first one.
    let area = find(items, "shapes.area");
    assert_eq!(sig(area), "function area(shape: Shape): number");
    assert!(
        area.docs
            .as_deref()
            .unwrap()
            .starts_with("The area of a shape.")
    );
    assert_eq!(
        sig(find(items, "shapes.double")),
        "function double(x: number): number"
    );
    assert_eq!(
        sig(find(items, "shapes.ORIGIN")),
        "const ORIGIN: Point = { x: 0, y: 0 }"
    );
    assert_eq!(
        find(items, "shapes.internal").visibility,
        Visibility::Private
    );
    // Exported under another name: documented under that name.
    assert_eq!(
        find(items, "shapes.default2").visibility,
        Visibility::Public
    );
    let geo = find(items, "shapes.geo");
    assert_eq!(geo.kind, ItemKind::Namespace);
    assert_eq!(
        find(items, "shapes.geo.hidden").visibility,
        Visibility::Private
    );
    assert_eq!(
        find(items, "shapes/util/strings.up").docs.as_deref(),
        Some("Upper.")
    );
    // No exports: a `.d.ts` of globals documents everything.
    assert_eq!(find(items, "globals.glob").visibility, Visibility::Public);
}

#[test]
fn doc_comments_are_cleaned() {
    assert_eq!(
        clean(
            "/**\n * One {@link Foo#bar}.\n *\n *   Indented {@link https://x.test | site}.\n */"
        ),
        "One {js:obj}`Foo.bar`.\n\n  Indented [site](https://x.test)."
    );
}

#[tokio::test]
async fn documents_a_package_with_references() {
    let dir = site("plugin", &[("src/index.ts", INDEX)]);
    let page = "```{js:currentmodule} shapes\n```\n\nSee {js:func}`area` and {js:class}`~shapes.Circle`.\n\n```{js:auto} double\n```\n";
    let plugin = plugin();
    let init = InitializeParams {
        site: SiteInfo {
            root: Some(dir.to_string_lossy().into_owned()),
            ..SiteInfo::default()
        },
        options: json!({"paths": [{"path": "src/index.ts", "module": "shapes"}]}),
        ..InitializeParams::default()
    };
    let declared = plugin.capabilities_for(&init).syntax;
    let (guide, diagnostics) =
        lsdocs_parser_markdown::parse_with_syntax("guide.md", page, &declared);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let ctx = context_with_parsers(
        &plugin,
        &init,
        vec![
            lsdocs_parser_markdown::plugin(),
            lsdocs_parser_rst::plugin(),
        ],
    );
    let out = run_transform_in(&plugin, &ctx, vec![guide]).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    for (doc, diagnostics) in &out {
        assert!(diagnostics.is_empty(), "{}: {diagnostics:?}", doc.source);
    }
    let sources: Vec<&str> = out.iter().map(|(d, _)| d.source.as_str()).collect();
    // A namespace gets a page, as modules do.
    assert_eq!(
        sources,
        [
            "guide.md",
            "api/js/index.md",
            "api/js/shapes.md",
            "api/js/shapes.geo.md"
        ]
    );
    let links = |nodes: &[Node]| -> Vec<String> {
        nodes
            .iter()
            .flat_map(Node::descendants)
            .filter_map(|n| match n {
                Node::Link(l) => Some(format!("{} -> {}", plain_text(&l.children), l.url)),
                _ => None,
            })
            .collect()
    };
    let (guide, _) = &out[0];
    assert_eq!(
        links(&guide.children),
        [
            "area() -> api/js/shapes.md#js.shapes.area",
            "Circle -> api/js/shapes.md#js.shapes.Circle"
        ]
    );
    assert!(guide.descendants().any(
        |n| matches!(n, Node::CodeBlock(c) if c.value == "function double(x: number): number")
    ));
    let (shapes, _) = &out[2];
    // `{@link}` in doc comments resolve; Markdown is parsed; tags are
    // sections.
    let all = links(&shapes.children);
    assert!(
        all.contains(&"area -> #js.shapes.area".to_owned()),
        "{all:?}"
    );
    let text = plain_text(&shapes.children);
    for expected in ["How big.", "The new radius.", "If negative.", "Use disks."] {
        assert!(text.contains(expected), "no {expected:?} in {text}");
    }
    assert!(
        shapes
            .descendants()
            .any(|n| matches!(n, Node::Admonition(a) if a.kind == "deprecated"))
    );
    assert!(
        shapes
            .descendants()
            .any(|n| matches!(n, Node::CodeBlock(c) if c.value.trim() == "area(new Circle(1))"))
    );
    assert!(!shapes.descendants().any(
        |n| matches!(n, Node::Section(s) if s.id.as_deref() == Some("js.shapes.Circle.hidden"))
    ));
    assert_eq!(TypeScript.domain(), "js");
}
