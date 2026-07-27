Find files by name pattern.

Takes `pattern` (required) and `hidden` to include hidden files. The pattern is
a glob such as `src/**/*.rs`, not a regular expression.

Supports `*`, `**` and `?`. Results are sorted and respect `.gitignore`.

Use this to locate files when you know roughly what they are called; use `grep`
when you know what is in them.
