# Source relationship review required before relicensing

This migration does not change the project license (GPL-3.0-only). Permissive
licenses of Iced/wgpu/cpal and LGPL FFmpeg DLLs alone do not establish that the
Rust implementation can be relicensed.

Before proposing a different project license, conduct a separate file-level
review of the Rust source, previous C++ implementation, contributor provenance,
third-party attribution and independently authored protocol behavior. Freeze
C++ `562120e4a6c85da1ec33bca88a6f438db91b8545` and Rust/Slint
`43bcb0ca8a7d659159290ff81bfa65ba3d627b9d` as review inputs. Identify copied or
translated expressive code versus independently implemented behavior, record
origins/evidence and author permissions, and resolve each relevant source's
license obligations. Do not infer independence from a language rewrite.

New migration modules (audio ring/device actor, media scheduler, session/status
modules, custom compositor/YUV primitive and desktop runtime) were authored for
this migration against public crate APIs. Existing protocol/crypto behavior and
retained references still require the separate review. This note is a review
boundary, not a completed provenance audit or new license grant. FairPlay
authorization analysis is outside this migration task.
