Read a file from the workspace.

Takes `path` (required), and `offset` and `limit` to read part of a large file.
`offset` is a 1-based line number.

Returns the file's contents with line numbers, so you can refer to a specific
line and use `edit` precisely.

Use `offset` and `limit` to page through a large file, or to read the rest of an
output that was truncated. Paths are relative to the workspace root.
