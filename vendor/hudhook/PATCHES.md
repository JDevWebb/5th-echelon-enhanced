# Patched hudhook 0.8.1

This is the published hudhook 0.8.1 crate (crates.io, git sha1 0ed5ab83), MIT licensed (see
`LICENSE`), used by the root `Cargo.toml`'s `[patch.crates-io]`. Only
`src/renderer/backend/dx9.rs` is changed, at the lines marked "5th Echelon patch" (plus a
`[lints.rust] non_snake_case = "allow"` at the end of `Cargo.toml`, because Cargo shows a path
dependency's warnings where it hid them for the crates.io copy):

- The DirectX 9 vertex buffer was created at `capacity * size_of::<DrawVert>()` (20 bytes a
  vertex) but filled with `CustomVertex` (24 bytes). A frame with between about 83% and 100% of
  the buffer's capacity in vertices overran it, and every such frame failed to render
  (`HRESULT(0x80004005)`), so the overlay vanished while still taking input. It's now sized by
  `size_of::<CustomVertex>()`; the then-unused `DrawVert` import is dropped. The index buffer was
  already sized by the type it's filled with (`DrawIdx`).
- When drawing failed, `?` returned before `StateBackup::restore`, leaving the game's D3D9 device
  state as the overlay set it. The state is now restored on every path after the backup.

Remove this folder and the patch entry once an upstream release has both fixes.
