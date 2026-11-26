# Changelog

## 0.6.3

- Two dialect mappings, installable by name: `urn:markdown:mapping:mdbook` (chapters and
  `SUMMARY.md`) and `urn:markdown:mapping:lectern` (slides, speaker and presenter notes,
  includes and ranges). Both lift the shared question and exercise directives (README,
  "Course directives").
- Only a YAML block on a document's first line is frontmatter. A `---`, text, `---` run
  later in a document was lifted as a second frontmatter, with its fields merged into the
  real one. It now lifts as a thematic break and a setext heading, as CommonMark reads it.
- The shape check counts distinct values. One fact asserted by two documents was counted
  twice against `sh:maxCount`.

## 0.6.2

- Optional arguments with invalid values are now refused instead of silently selecting the default. No persisted state changes are needed when upgrading.
