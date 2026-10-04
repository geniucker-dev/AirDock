# Supplemental Cargo license texts

`index.json` maps exact locked crate versions to their immutable upstream source
commit and copied license/copyright notices, with original URLs and SHA-256 hashes.
These crates omit their workspace license files in their published Cargo packages.
The distribution includes these texts alongside licenses found in Cargo sources.
Changing a crate version or source commit requires refreshing its supplement;
packaging fails instead of silently omitting a required text.

`ffmpeg-sys-next` declares WTFPL and `hexf-parse` declares CC0 in their upstream
README; both declarations accompany the full corresponding SPDX license text.
Those canonical texts are pinned to the immutable license-list-data commit cited
in the index. Iced's icons are covered by its MIT text. Iced's optional Fira Sans
feature is disabled in this dependency graph.
