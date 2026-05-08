## Learned User Preferences

## Learned Workspace Facts

- The IDE repo is primarily a Cargo workspace; there is typically no root `package.json`, so Node-based tooling (`npx`, shadcn CLI, npm scripts) only works inside a JS/TS subproject directory, not at the Rust workspace root.
- The same repo may appear as `/home/jack/phazeai_ide` in the editor while another checkout exists at `/media/jack/New Volume/ShadowStorage/Dev/phazeai_ide`; Cursor hook state paths (including continual-learning) may reference the ShadowStorage checkout.
