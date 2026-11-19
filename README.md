# lsdocs-apidoc-typescript

Part of [lsdocs](https://github.com/lsdocs), a static-site generator whose
parsers, transformers and renderers are plugins.

`crates/apidoc-typescript` (`lsdocs-apidoc-typescript`, builtin
`apidoc-typescript`): documents TypeScript (and JavaScript with JSDoc)
from source with the oxc parser, in the `js` domain. Its MSRV is 1.97
(oxc's crates).

The plugin's settings and output are described in the lsdocs
documentation (https://lsdocs.github.io, "apidoc-typescript").

## Known limitations

- Re-exports (`export * from`, `export { x } from`) and `export =` are
  not followed.
- JSDoc type annotations in plain JavaScript (`@param {T}`) are kept as
  documented text, not as signatures.

## TODO

- [ ] Re-exports and `export =`.
- [ ] JSDoc types as signatures in plain JavaScript.
