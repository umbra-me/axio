Read a file from the workspace.

Returns the file's contents with line numbers, so you can refer to a specific
line and use `edit` precisely.

Use `offset` and `limit` to page through a large file, or to read the rest of an
output that was truncated. Paths are relative to the workspace root.
