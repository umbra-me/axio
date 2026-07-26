Run a shell command in the workspace.

Use this for builds, tests, linters, version control and anything else the
project already has a command for. Prefer a project's own tooling over
reimplementing what it does.

The command runs with the workspace as its working directory. Output is
truncated if it is very large, and the full output is written to a file you can
read.

Some commands need approval. Depending on how axio was started, approval is
either requested from the person running it or decided without asking. **A
refusal is final: it will not become an approval if you try again.** Do not
re-send a refused command, with or without changed arguments, and never report
a result for a command that did not run.

A command that is one program with arguments (`git status`, `cargo test`) can
be permitted by a rule. A pipeline, a sequence, a redirect or a substitution
cannot match any rule and always needs explicit approval — so where the work
can be done as separate simple commands, issue them separately.
