# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(under Cargo's 0.x reading: while `0.x`, a breaking change bumps the minor and a
new feature is a patch).
## [0.5.1] - 2026-06-30



## [0.5.0] - 2026-06-30

### Added

- **scene:** Capture [connection] sections (signal/from/to/method + spans)
- **scene:** Capture node body property keys + spans
- **ide:** Classify a scene node as a renameable symbol (GodotDef::SceneNode)
- **ide:** Classify the full scene-node reference cascade (parent=, connection, get_node)
- **scene:** Decode \u/\U Unicode + \a\b\f\v escapes in .tscn strings
- **scene:** Suppress cascading DanglingParent on a detached subtree

### Documentation

- Enforce missing_docs on 6 crates + internal-API banners (Stage 9b/9c)



## [0.4.0] - 2026-06-28



## [0.3.0] - 2026-06-28



## [0.2.1] - 2026-06-27

### Added

- **hir:** Type node paths that descend INTO an instanced sub-scene ($Enemy/Sprite)

### Style

- Apply rustfmt to the Phase-5 hardening changes



## [0.2.0] - 2026-06-27

### Documentation

- Client-facing READMEs + accurate v0.1 package metadata


