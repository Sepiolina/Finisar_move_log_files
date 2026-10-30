# `.github/workflows/build.yml`

[Reference index](../../README.md) · [Source](../../../../.github/workflows/build.yml)

GitHub Actions workflow triggered on main pushes/PRs, version tags, and manual dispatch. Ubuntu check job installs Tauri dependencies, runs workspace Clippy with warnings denied, then workspace tests. Independent Windows release-build job compiles the workspace; version-tag builds create a GitHub release with release notes and executable assets. Repository content-write permission supports release publishing. Contains workflow steps rather than application functions.
