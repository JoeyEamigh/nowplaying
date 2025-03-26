build:
  cargo build --release
  bun run build

bindings:
  bun run build

example:
  bun run dev
  cd bindings && bun run example

tokei:
  tokei -t Rust,TypeScript,TSX

version:
  ./scripts/set-versions.sh