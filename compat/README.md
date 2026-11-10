# gpui-pre compatibility crates

[GPUI Kit](https://github.com/longbridge/gpui-kit) depends on GPUI through the
`gpui-pre-*` snapshots on crates.io, and Cargo's `[patch]` only replaces a
package with one of the same name and a version that meets the requirement.
The crates here carry exactly those names and the version GPUI Kit pins, and
each re-exports this repository's crate of the same library name
(`pub use gpui_fast::*`), so an application on GPUI Kit, from crates.io or git
alike, runs on gpui-fast with:

```toml
[patch.crates-io]
gpui-pre = { git = "https://github.com/longbridge/gpui-fast" }
gpui-pre-platform = { git = "https://github.com/longbridge/gpui-fast" }
gpui-pre-macros = { git = "https://github.com/longbridge/gpui-fast" }
gpui-pre-sum-tree = { git = "https://github.com/longbridge/gpui-fast" }
# and gpui-pre-web / gpui-pre-reqwest-client when the graph has them
```

Each crate exposes the same features as the gpui-pre crate it stands in for,
forwarded to ours. Nothing else lives here: gpui-fast's own crates keep
upstream's names and versions.

Things to keep in mind:

- The version is the snapshot GPUI Kit pins, `=0.3.12` today. When GPUI Kit
  moves to a newer `gpui-pre`, move every crate here with it; until then Cargo
  warns that the patch was not used and keeps the snapshot.
- gpui-pre rewrites the `gpui::` paths its macros emit to GPUI Kit's when the
  crate being compiled depends on it. `gpui_macros` does the same with its
  `gpui-pre-facade` feature, which `gpui-pre-macros` here turns on, so a crate
  on GPUI Kit alone needs no alias for `#[gpui_kit::test]` and the derives.
- An application on GPUI Kit that keeps its own `workspace-hack` (cargo-hakari)
  regenerates it after adding the patch: one generated against crates.io pins
  the `gpui-pre-macos` / `gpui-pre-apple` snapshots, which then build against
  this repository's `gpui` and fail.
- GPUI Kit builds against this crate's public API as upstream's, so a change
  here must not break it: no new fields on public structs that others build
  with literals (`ContentMask`, `Hitbox`), no changed method signatures. Add a
  type or a method instead, as `ClipMask` and `Window::with_clip_mask` do for
  rounded masks.
