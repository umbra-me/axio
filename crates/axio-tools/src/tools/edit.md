Replace an exact string in a file.

`old` must appear exactly once. If it appears more than once the edit is
refused rather than guessing — include surrounding lines to make it unique.

This is the tool to reach for when changing existing code: the change is
reviewed as a diff, and a unique anchor is safer than a line number that moves.
