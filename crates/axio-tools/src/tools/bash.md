Run a shell command in the workspace.

Use this for builds, tests, linters, version control and anything else the
project already has a command for. Prefer a project's own tooling over
reimplementing what it does.

The command runs with the workspace as its working directory. Output is
truncated if it is very large, and the full output is written to a file you can
read. Commands that need approval will ask before running.
