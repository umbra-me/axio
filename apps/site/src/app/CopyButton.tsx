"use client";

import { useEffect, useRef, useState } from "react";

/**
 * Copies one command to the clipboard.
 *
 * Rendered only after mount, and only where the Clipboard API actually exists:
 * it is unavailable on plain http and in older browsers, and a button that
 * silently does nothing is worse than no button. Nothing else on the page needs
 * JavaScript to work, and this keeps that true — the command is still there to
 * select by hand.
 */
export default function CopyButton({ text, what }: { text: string; what: string }) {
  const [ready, setReady] = useState(false);
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => {
    setReady(typeof navigator !== "undefined" && !!navigator.clipboard);
    return () => window.clearTimeout(timer.current);
  }, []);

  if (!ready) return null;

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Permission can be refused even where the API exists. Saying nothing is
      // right: the label stays "copy", so the button reads as not having worked,
      // which is the truth.
      return;
    }
    setCopied(true);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setCopied(false), 2000);
  }

  return (
    <button
      type="button"
      className="terminal__copy"
      onClick={copy}
      // The visible label is two characters of uppercase mono; the accessible
      // name says which of the three commands this is.
      aria-label={`Copy the ${what} command`}
      data-copied={copied || undefined}
    >
      <span aria-hidden="true">{copied ? "copied" : "copy"}</span>
      <span className="visually-hidden" role="status">
        {copied ? "Copied to clipboard" : ""}
      </span>
    </button>
  );
}
