Search file contents with a regular expression.

Takes `pattern` (a regular expression, required), `glob` to restrict which
files are searched, and `hidden` to include hidden ones. It takes no other
arguments: there is no `query`, no `path` and no result limit.

Returns matching lines with their file and line number. Respects `.gitignore`.
Narrow the search with `glob` to a subset of files when you can — it is faster
and the output is easier to read.
