Write a file, creating it or replacing it entirely.

Prefer `edit` when changing part of an existing file: a write replaces the whole
thing, and a diff of a small change is easier to review than a whole file.

Parent directories are created as needed. Paths are relative to the workspace
root.
