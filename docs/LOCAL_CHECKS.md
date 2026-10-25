# Local verification

Automated CI is disabled for this project. There are no self-hosted runner
requirements. Run the applicable checks on the device where development happens,
before opening or merging a pull request. Keep the existing checks and tests;
removing CI does not waive verification.

Run from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Install stable Rust and the native build prerequisites for your development platform. Run macOS-specific checks and manual application smoke tests on macOS; a pass on one platform does not establish cross-platform coverage.

Record the commands run and their results in the pull request. State any skipped
checks, unavailable fixtures, or untested platforms explicitly.
