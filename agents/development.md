# development

follow [architecture and boundaries](architecture.md) when designing or changing code.

keep changes within the task and preserve unrelated work. add or update meaningful tests when changed behavior needs coverage.

choose verification based on the affected behavior and its callers. start with focused tests and checks for the changed package. broaden checks when shared contracts, dependencies, workspace configuration, or behavior across packages are affected, or when the impact is unclear. documentation-only changes usually need source and link checks.

run workspace checks from the repository root when the change warrants them:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

fix failures introduced by the change. report what was checked and any relevant gaps.
